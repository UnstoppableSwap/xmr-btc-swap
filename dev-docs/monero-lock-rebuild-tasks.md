# Monero lock rebuild follow-up tasks

- [x] Restore recovery when the initial `BtcLocked` restore-height lookup fails.
  Retry exhaustion enters `BtcEarlyRefundable`; observed cancellation expiry enters `SafelyAborted`.
  Rebuilds enter `XmrReadyToLock`, so this initial lookup precedes construction/publication of a Monero lock transaction.

## Next items

- [x] Require a configurable confirmation depth before abandoning an original lock transaction (#5).
  `monero.lock_rebuild_confirmations` defaults to 15 and must be positive.
  Search block input key images from the preserved restore height and recheck canonical depth before and after scanning the shared wallet.
  This reduces shallow-reorg risk; it does not make a conflicting spend irreversible.
- [x] Cancel live scans in `XmrReadyToLock` when the Bitcoin cancel timelock expires (#6b).
  Both pre-construction and pre-early-refund scans are raced against cancellation and lead to `SafelyAborted` when cancellation wins.
  Interrupted scans never establish emptiness or authorize early refund.
  This preserves the ready state's existing abort policy; the deferred recovery/finality caveats below still apply.
- [x] Keep supervising ready-state recovery when received outputs or scan failures prevent construction/early refund (#4).
  Enter `WaitingForCancelTimelockExpiration` with no transfer proof and stop reserving additional Monero.
  Carry the optional proof through Bitcoin cancellation and key recovery.
  Without a proof, stop in `XmrRefundable` before retrying Monero refund construction; preserve the refund key and restore height.
  Existing populated proofs remain readable; older binaries cannot read new recovery states with null proofs.
- [x] Refresh obsolete mempool snapshots when transactions disappear between hash enumeration and fetching (#7).
  Retry the whole mempool scan with fresh hashes under the supplied `inner_retry` budget, without per-batch retries.
  `None` means one attempt; a changed tip height or hash aborts retries immediately.
  Block scanning remains in the caller.

## Deferred: wallet-wide recovery and restore-height restructuring (#4)

- [ ] Investigate capturing the restore height during swap setup and carrying it in `State3`.
  Preserve compatibility with existing persisted swaps; never replace an unknown historical height with the current tip.
- [ ] Generalize received-output discovery, retaining an early-exit existence-check wrapper.
  Distinguish mempool detections from confirmed outputs with usable blockchain indices.
- [ ] Support recovery without a transfer proof by discovering outputs and refunding the mature, unspent subset.
  Review multi-output sweep construction, spent detection, dust, state serialization, and manual recovery commands.
- [ ] Make the recovery distinction between fresh construction and rebuilding explicit.
  Rebuilding enters `XmrReadyToLock` with the original restore height.
  Review early refund and safe-abort eligibility there: a previously published original transaction may become valid again after a reorg removes its conflicting spend.
