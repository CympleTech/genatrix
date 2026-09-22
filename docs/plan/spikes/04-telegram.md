# Spike 04 · Telegram over the user-account protocol

**Question** (design doc 10): can a Rust MTProto client sign in with a phone number and two-step verification, list the dialogs, and page through a large group's history, ten thousand messages, without hitting Telegram's limits?

**Verdict: PASS.** Not a spike script: the real connector, run for the first time against the author's account. Sign-in, dialogs, history and live updates all worked on the first attempt; no flood wait was reported.

## Environment

- Mac mini, Apple M2 Pro, 16 GB; `grammers-client` 0.10 with a session held in memory and persisted as JSON in the encrypted store
- The author's own account: 170 conversations (direct chats, groups, channels)
- The connector in its own sandboxed process, talking to the core over the connector protocol, like the mail connector

## Results

| Check | Threshold | Result |
|---|---|---|
| Sign in: number, code, password | all steps complete | **yes**, from the terminal, first attempt |
| Dialog list | complete | **170 conversations** in about five seconds |
| History paging | 10,000 messages without a limit error | **10,744 messages in the first four minutes**, about 3,000 a minute, no flood wait at that point; the full history was still coming in when this was written |
| Live updates | new messages arrive | the stream is up with catch-up |

## What the real account taught

1. **The library's 0.10 release changed shape**: connections live behind a sender pool that is run as its own task, and sessions are a trait with in-memory and `SQLite` storages. Neither storage suits a session that has to be protected like a password, so the connector keeps its own in-memory session and hands the core a serializable snapshot, kept in the encrypted store beside the sync cursors. Design 05 said "keychain"; the store is protected by the keychain's master key and can hold the peer cache too, which is large. The design was amended.
2. **A dependency of the crypto crate had moved on** (`glass_pumpkin` 2.0.0-rc1 against a crate written for rc0); the workspace pins rc0 in `Cargo.lock`.
3. **The sandbox is weaker here than for mail**: Telegram speaks on port 443, and a port filter that allows 443 allows it to anywhere. The datacenter addresses are in the capability and checked by the connector; the OS still keeps the process off every other port, out of the data directory and the keychain files, and from starting any other program. Design 05 says so.

## Impact on the design

- Design 05: session storage, and the honest sandbox statement, both recorded.
- Design 01 and 05: an edit arrives as the same message with different bytes and becomes a new version of the item, as designed; a deletion is not yet a tombstone.
- Media: described (kind, name, type, size) but not fetched in this phase; the 10 MB rule for immediate download is still to build.
