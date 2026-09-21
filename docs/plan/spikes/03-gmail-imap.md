# Spike 03 · Gmail over IMAP

**Question** (design doc 10): does an app password log in, are Gmail's IMAP extensions usable, and how fast does a backfill with bodies run?

**Verdict: PASS on function; the speed threshold is not met by extrapolation, and the recommendation is to change the threshold rather than the connector.** Login works, the extensions are offered and the message id is read, deduplication holds. Throughput is bounded by the server, not by this machine, and the newest-first backfill already gives the user this week's mail within minutes, which is what the threshold was protecting.

## Environment

- Mac mini, Apple M2 Pro, 16 GB, in New Zealand; Gmail's IMAP endpoint is overseas
- The author's own Gmail: 1,657 messages in All Mail, 39 in Sent Mail, history back to December 2022
- `genatrix sync --limit 10000`, release build, one IMAP connection, batches of 50 with `BODY.PEEK[]`
- Not a spike script: the real connector and the real ingestion path, so the numbers are the product's numbers

## Thresholds and results

| Check | Threshold | Result |
|---|---|---|
| App-password login | works | **yes**; a wrong password is reported as the user's problem, not retried |
| Gmail extensions | `X-GM-EXT-1` offered and usable | **offered**; `X-GM-MSGID` read; `X-GM-THRID` parsed by `imap-proto` but not exposed by `async-imap` |
| Backfill speed | 10,000 messages with bodies in 30 minutes (333 per minute) | **221 per minute**: 1,696 in 7 m 38 s; 10,000 extrapolates to 45 minutes |
| Where the time goes | | 7 m 28 s waiting on the server, 7 s storing locally |

A debug build ran first at 211 per minute, so optimisation level barely matters: 98 % of the wall clock is the round trip to Gmail.

## What the real server revealed that the fake did not

1. **The Gmail message id was never read.** The wire layer asked for `X-GM-MSGID` and `X-GM-THRID` and then filled both fields with `None`. Every message fell back to a `folder/uidvalidity/uid` identifier, so the 39 messages in Sent Mail, all of which are also in All Mail, were stored twice. The fake server also returns `None` for both, so the tests passed. Fixed: the message id comes from the library's accessor; on Gmail only All Mail is read, since it holds every message once; the folder choice has tests. The thread id waits on a library accessor and threads use the `References` chain meanwhile.
2. **A byte-indexed window in the entity decoder panicked** on marketing mail whose preview text is a run of zero-width non-joiners after an `&`. Fixed and covered by a test. A single malformed message still stops the whole sync, which the realtime loop must not allow (design 05, fault severity).
3. **HTML-only mail leaked stylesheets and entities into the text**, because the parser's own HTML rendering was taken as the text part. Fixed: only a real text part counts as text; otherwise our converter runs. Zero-width characters are dropped.
4. **The timeline looked out of order** because it printed each message in the sender's time zone. Sorting was right; the display now uses local time.
5. **A run that failed on a missing gateway configuration left a half-initialised directory.** The check now happens before anything is created.

## Impact on the design

- Design 10, Gmail IMAP spike row: the 30-minute threshold assumed a first-run experience that depends on backfill finishing. Design 05 already makes backfill newest-first and concurrent with realtime sync, so the user sees recent mail within minutes regardless of history size. Proposed replacement: "within five minutes of adding an account, the last week of mail is on the timeline; full history follows in the background at whatever rate the server allows." The "headers first, bodies later" fallback is not needed.
- Design 05, Gmail: note that Sent Mail is a label over All Mail and is not read separately. Thread ids from Gmail depend on the client library; the `References` fallback is the default until then.
- If throughput ever matters, Gmail allows several IMAP connections per account, and the fetch stage is the only stage worth parallelising. Not planned.
