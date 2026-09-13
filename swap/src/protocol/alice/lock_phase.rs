//! Serialization of the Monero lock phase across concurrent swaps.
//!
//! wallet2 only marks an output spent once its lock transaction is relayed and
//! monero-sys has no reserve API, so two overlapping swaps can pick the same
//! output and the loser's lock transaction becomes a permanent double-spend
//! that monerod rejects forever. [`MoneroLockPhase`] hands out one process-wide
//! permit for the phase; each swap tracks its participation through a
//! [`LockPhaseSession`] so the state machine only has to say whether its
//! current state is inside the phase.

use std::time::{Duration, Instant};

use tokio::sync::{Mutex, MutexGuard};

/// Process-wide serialization of the Monero lock phase.
///
/// The phase spans output selection (`BtcLocked`) through the first relay of
/// the lock transaction (`XmrLockTransactionConstructed`).
pub struct MoneroLockPhase {
    mutex: Mutex<()>,
    max_hold: Duration,
}

impl MoneroLockPhase {
    /// A lock phase whose sessions may hold the permit for at most `max_hold`.
    pub fn new(max_hold: Duration) -> Self {
        Self {
            mutex: Mutex::new(()),
            max_hold,
        }
    }

    /// Start tracking one swap's participation in the phase.
    pub fn session(&self) -> LockPhaseSession<'_> {
        LockPhaseSession {
            phase: self,
            held: None,
            abandoned: false,
        }
    }
}

/// One swap's view of the serialized phase, held on `run_until`'s stack because
/// construct and publish are separate states with a persist in between.
pub struct LockPhaseSession<'a> {
    phase: &'a MoneroLockPhase,
    held: Option<(MutexGuard<'a, ()>, Instant)>,
    /// Set once this swap exhausts `max_hold` and continues unserialized.
    abandoned: bool,
}

impl LockPhaseSession<'_> {
    /// Bring the session in line with whether the swap's current state is
    /// inside the phase: acquire the process-wide permit on entry (unless this
    /// swap already overstayed and continues unserialized), release it on exit.
    pub async fn sync_to(&mut self, in_phase: bool) {
        if !in_phase {
            self.release();
        } else if self.held.is_none() && !self.abandoned {
            let guard = self.phase.mutex.lock().await;
            tracing::debug!("Entered the serialized Monero lock phase");
            self.held = Some((guard, Instant::now()));
        }
    }

    /// Remaining time this swap may keep the permit, or `None` when it is not
    /// holding it (never entered, already released, or abandoned).
    pub fn deadline(&self) -> Option<Duration> {
        self.held
            .as_ref()
            .map(|(_, since)| self.phase.max_hold.saturating_sub(since.elapsed()))
    }

    /// Whether this session currently holds the process-wide permit.
    pub fn holds_permit(&self) -> bool {
        self.held.is_some()
    }

    /// Leave the phase normally: release the permit and re-arm the session for
    /// a future phase.
    pub fn release(&mut self) {
        if self.held.take().is_some() {
            tracing::debug!("Left the serialized Monero lock phase");
        }
        self.abandoned = false;
    }

    /// Give up the permit after overstaying the deadline. The swap continues
    /// unserialized until it leaves the phase, which re-arms it via
    /// [`LockPhaseSession::release`].
    pub fn abandon(&mut self) {
        self.held = None;
        self.abandoned = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WAIT: Duration = Duration::from_millis(50);

    #[tokio::test]
    async fn phase_is_exclusive_until_released() {
        let phase = MoneroLockPhase::new(Duration::from_secs(60));

        let mut first = phase.session();
        first.sync_to(true).await;
        assert!(first.holds_permit());

        let mut second = phase.session();
        assert!(
            tokio::time::timeout(WAIT, second.sync_to(true))
                .await
                .is_err(),
            "second session must wait while the first holds the permit"
        );
        assert!(!second.holds_permit());

        first.release();
        assert!(!first.holds_permit());

        tokio::time::timeout(WAIT, second.sync_to(true))
            .await
            .expect("second session acquires once the first released");
        assert!(second.holds_permit());
    }

    #[tokio::test]
    async fn deadline_is_bounded_by_max_hold_and_none_when_not_held() {
        let phase = MoneroLockPhase::new(Duration::from_secs(60));
        let mut session = phase.session();

        assert_eq!(session.deadline(), None);

        session.sync_to(true).await;
        let deadline = session.deadline().expect("held sessions have a deadline");
        assert!(deadline <= Duration::from_secs(60));

        session.sync_to(false).await;
        assert_eq!(session.deadline(), None);
    }

    #[tokio::test]
    async fn deadline_expires_to_zero() {
        let phase = MoneroLockPhase::new(Duration::from_millis(5));
        let mut session = phase.session();

        session.sync_to(true).await;
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(session.deadline(), Some(Duration::ZERO));
    }

    #[tokio::test]
    async fn abandoned_session_continues_unserialized_until_it_leaves_the_phase() {
        let phase = MoneroLockPhase::new(Duration::from_secs(60));

        let mut wedged = phase.session();
        wedged.sync_to(true).await;
        wedged.abandon();
        assert!(!wedged.holds_permit());
        assert_eq!(wedged.deadline(), None);

        // Still inside the phase: the session must not re-acquire...
        tokio::time::timeout(WAIT, wedged.sync_to(true))
            .await
            .expect("an abandoned session never blocks");
        assert!(!wedged.holds_permit());

        // ...so another swap is free to take the permit meanwhile.
        let mut other = phase.session();
        tokio::time::timeout(WAIT, other.sync_to(true))
            .await
            .expect("the permit is free after an abandon");
        assert!(other.holds_permit());
        other.release();

        // Leaving the phase re-arms the abandoned session for the next one.
        wedged.sync_to(false).await;
        tokio::time::timeout(WAIT, wedged.sync_to(true))
            .await
            .expect("a re-armed session acquires again");
        assert!(wedged.holds_permit());
    }

    #[tokio::test]
    async fn dropping_a_session_frees_the_permit() {
        let phase = MoneroLockPhase::new(Duration::from_secs(60));

        {
            let mut held = phase.session();
            held.sync_to(true).await;
            assert!(held.holds_permit());
        }

        let mut next = phase.session();
        tokio::time::timeout(WAIT, next.sync_to(true))
            .await
            .expect("dropping a holding session frees the permit");
        assert!(next.holds_permit());
    }
}
