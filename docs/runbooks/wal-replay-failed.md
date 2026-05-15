# Runbook: `WAL replay failed` at boot

A boot panic with message starting `WAL replay failed:` or `invariant 21
violated:` means Stage 1b's replay step (server lib.rs Step 3.6 / 3.8)
rejected the on-disk WAL. The server refuses to start to prevent silent
state divergence.

## Identify the failure mode

Grep the boot stderr for the exact panic line. Five variants:

| Panic message prefix                          | Root cause                                     |
| --------------------------------------------- | ---------------------------------------------- |
| `WAL writer open failed: corrupt at sequence` | CRC mismatch in mid-stream segment (writer-open level — see spec §7.1). |
| `WAL replay failed: sequence gap at expected` | A WAL record is missing between two existing records. |
| `WAL replay failed: rkyv decode at sequence`  | An event's bytes do not match the current rkyv layout. Usually a schema change without WAL clear. |
| `WAL replay failed at sequence`               | `apply_event` returned an `ApplyError` (UnknownOrder / DuplicateOrder / OverFill / UnexpectedVariant). |
| `invariant 21 violated`                       | Reader's replayed event count ≠ writer's recorded next sequence. WAL state internally inconsistent. |

## Recovery actions

**Dev / CI environment (Stage 1b ships into this).** The contract is:
no production data exists. Reset:

```bash
# Stop the failed server (it's already exited).
rm -rf data/wal

# (Optional) preserve the broken WAL for forensics:
mv data/wal data/wal-broken-$(date +%s)
mkdir data/wal

# Restart.
EXG_CONFIG=config/default.toml RUST_LOG=info ./target/release/exg-server
```

The server boots with an empty WAL, replays zero events, and proceeds
normally. All previously open orders are lost.

**Production environment (future, not Stage 1b).** Do not run the dev
recovery. Page on-call SRE. Steps will live in a separate Stage 5+
runbook covering snapshot fallback, partial-truncate repair, and
multi-replica reconciliation. The Stage 1b runbook is intentionally
narrow.

## Most common cause in dev: schema drift

After pulling a Stage 1b commit that bumped an event schema (the
Stage 1a → 1b cutover is the classic case), any WAL written by the
older binary is unreadable. The fix is always `rm -rf data/wal`.
See spec §9.5 "Migration from Stage 1a."

## What NOT to do

- Don't delete only "some" segments — the writer's `recover_state` will
  surface a CRC or sequence-gap panic.
- Don't manually edit a segment file — the CRC trailer makes byte edits
  detectable, but a coincidental CRC-valid edit (vanishingly rare) would
  produce a silently wrong replay.
- Don't disable invariant 21 to "see what happens" — it exists because
  reader/writer disagreement is a real-world symptom of a bug somewhere.
  File an issue instead.
