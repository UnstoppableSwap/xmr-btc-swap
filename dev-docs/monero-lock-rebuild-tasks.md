# Monero lock rebuild follow-up tasks

- [x] Restore recovery when the initial `BtcLocked` restore-height lookup fails.
  Retry exhaustion enters `BtcEarlyRefundable`; observed cancellation expiry enters `SafelyAborted`.
  Rebuilds enter `XmrReadyToLock`, so this initial lookup precedes construction/publication of a Monero lock transaction.

## Next items

- [x] Require a configurable confirmation depth before abandoning an original lock transaction (#5).
  `monero.lock_rebuild_confirmations` defaults to 10 and must be positive.
  Search block input key images from the preserved restore height and recheck canonical depth before and after scanning the shared wallet.
  This reduces shallow-reorg risk; it does not make a conflicting spend irreversible.
- [ ] Bound or cancel live scans in `XmrReadyToLock` without treating an interrupted scan as empty (#6b).
  The constructed-state publication/rebuild work is already raced against Bitcoin cancellation.
  Ready-state recovery must account for the distinction between fresh construction and rebuilding.
- [ ] Refresh obsolete mempool snapshots when transactions disappear between hash enumeration and fetching (#7).
  Reconcile the chain as well; do not silently ignore missing transactions.

## Deferred: wallet-wide recovery and restore-height restructuring (#4)

- [ ] Keep supervising ready-state recovery when received outputs or scan failures prevent construction/early refund.
  Do not leave liquidity permanently reserved after Bob cancels/refunds.
- [ ] Investigate capturing the restore height during swap setup and carrying it in `State3`.
  Preserve compatibility with existing persisted swaps; never replace an unknown historical height with the current tip.
- [ ] Generalize received-output discovery, retaining an early-exit existence-check wrapper.
  Distinguish mempool detections from confirmed outputs with usable blockchain indices.
- [ ] Support recovery without a transfer proof by discovering outputs and refunding the mature, unspent subset.
  Review multi-output sweep construction, spent detection, dust, state serialization, and manual recovery commands.
- [ ] Make the recovery distinction between fresh construction and rebuilding explicit.
  Rebuilding enters `XmrReadyToLock` with the original restore height.
  Review early refund and safe-abort eligibility there: a previously published original transaction may become valid again after a reorg removes its conflicting spend.
