# Kickoff

The eleven design documents in `docs/design/` are final. This page is the bridge from design to code: what happens first, who does it, and what it needs.

Design documents are written in Chinese for discussion with the author. Everything else in this repository, including this directory, code comments, and commit messages, is in English.

## Order of work

1. **Four technical spikes**, one day each, with pass thresholds defined in design doc 10. If any spike fails, the design changes before any code is written.
2. **Phase 0**: this machine, model selection, repository skeleton, gateway, storage. No real data enters the system during Phase 0.
3. **M1 · Collect**: the IMAP connector and the timeline. Product code starts here.

## Phase 0 status

| Task | State |
|---|---|
| Repository skeleton, CI, lints | done |
| `genatrix-model`: entities, sensitivity, effective level | done |
| `genatrix-keys`: database and ticket keys | done |
| `genatrix-store`: SQLCipher, migrations, FTS5 trigram, timeline queries, export/import | done |
| `genatrix-ledger`: append-only hash-chained ledger with a head file | done |
| `genatrix-infer`: sandboxed MLX inference over a Unix socket | done |
| `genatrix-llm`: egress tickets, policy, registry, gateway | done; cloud transport deferred to phase two |
| `genatrix-gate`: content patterns, rule file, redaction, the egress gate | done |
| `genatrix-agent`: isolation, tool protocols, output collars, actions, run context | done |
| `genatrix-daemon`: the layers assembled, the gate-to-gateway caller, the classification pipeline | done |
| Local web interface: timeline, search, item detail, records | done; read-only until there are actions to approve |
| `genatrix-keys`: HKDF derivation from one master key | done |
| `genatrix-store`: encrypted content-addressed files for raw records and attachments | done |
| `genatrix-connector`: capabilities, checkpoints, backfill, fault severity | done |
| `genatrix-connector-imap`: normalization, the sync engine against a fake server, the IMAP wire layer | done; verified against Gmail |
| Accounts, mail ingestion, `account` and `sync` commands | done |
| Realtime sync: persisted cursors, one connection alternating catch-up and backfill, IDLE with a polling fallback, reconnection with backoff, per-account state in `serve` and the page | done; the seven-day soak runs on the author's machine from the day the service was installed |
| Sign-in: `account --add` verifies the password against the server and keeps it in the keychain; `--forget` removes it; the address becomes a handle of the user's own person | done; via `/usr/bin/security` until the signed application shell |
| `service install`: a launchd agent that runs the shell, or `serve` alone, from login onwards; a crash restarts it, a quit from the menu does not | done |
| Master key in the keychain | done for the default data directory, with a one-time move from the old key file; a `--data-dir` development directory keeps the key in a file and says so |
| `reprocess`: derive every item again from its raw record with today's normalization, in place | done |
| Connector in its own sandboxed process: length-prefixed protobuf over `run/core.sock`, a one-time token, a `sandbox-exec` profile that allows only the granted ports, no data directory, no keychain files, no other programs | done; the OS enforces ports, the connector checks hosts (design 05 says so) |
| Gmail thread id | done, through a one-accessor patch carried in `vendor/async-imap` |
| Menu bar shell (`apps/menubar`, Swift): four looks, one line per account, open, quit; runs the core and is what `service install` installs when present | done as a plain executable; the signed application bundle and the first-run screens (design 09) are phase-two packaging |
| crabllm vendored and patched | done |
| Model selection against a real evaluation set | not started; needs ingested data |

## M2 status

Design 10, "理解". Done when every item has a level and a summary, vector search works, the records page shows that nothing has left the device, and the author agrees with more than 90 of 100 sampled judgements.

| Task | State |
|---|---|
| The core starts and supervises the inference process (sandboxed) and the gateway; the ticket key is generated per run and left in `run/ticket.key` for the other commands | done |
| Classification runs inside `serve` as mail arrives | done. Measured on the real mailbox: the model reads about 150 prompt tokens a second, so the prompt carries sender, subject and the first 200 characters without links, about 1,300 tokens for a batch of ten, 9 seconds a batch, under a second an item; whole bodies took 18 seconds a batch. Quality not yet measured |
| Review on the page: the model's judgement of a message, confirmed or corrected with one click; the tally of agreement; a level chooser on every item (design 06) | done; the author's judgements are the evaluation set (design 04) |
| Model selection against a real evaluation set: the author's mailbox, 100 sampled judgements | waiting on the author's review pass |
| Chunking, embedder role, `sqlite-vec`, vector search on the page | done: `multilingual-e5-small` served by the inference process on the CPU with Apple's Accelerate (candle; its Metal backend lacks layer-norm; about 2,000 prompt tokens a second, 32 chunks in 8 seconds), chunks of about 700 characters at paragraph and sentence boundaries, vectors in `chunk_vec` beside the Embedding annotations, search merges full-text and nearest-by-meaning hits into one timeline |
| Summarizer role; summary shown per item | done: messages of 280 characters or more get up to three lines in their own language, shown under the text marked as the model's; shorter ones are their own summary |
| The core starts the model side | done: two models, one sandboxed process; `service install` falls back to `launchctl load` where `bootstrap` needs a GUI session |
| Records page backed by the ledger for every model call | partly: the page reads the ledger; the item count of model calls will grow with the pipelines |
| Telegram connector (non-blocking; due by the end of M3) | written: sign-in from the terminal (`account --add-telegram`), dialogs, history newest first per conversation, live updates with catch-up, edits as new versions, media described but not fetched; its own sandboxed process over the same protocol. Run against the author's account: 170 conversations, thousands of messages a minute, no flood wait. Direct chats are read whole, groups and channels the last thirty days (design 05); deletions are not yet tombstones and media is not fetched |

## M3 status

Design 10, "摘要". Done when there has been a digest every morning for fourteen days, at least half its lines useful, "you promised" has caught one thing the author had really forgotten, and mail and Telegram share one timeline.

| Task | State |
|---|---|
| Daily digest pipeline: the last 24 hours, sorted by the model into needs-reply, worth-knowing, skip, one line each in the message's language, every line with its source; made after eight each morning, or with `genatrix digest` | done; quality to be judged over the fourteen days |
| Commitment extraction: explicit promises in mail and chat, by the user or to the user, as inferred commitments with evidence; confirm, done, or reject on the page | done |
| The Today page: digest groups with sources that open in place, open promises with their evidence and the user's say | done as the first screen; pending approvals arrive with M4 |
| Person page and relationship statistics | done: everyone the user exchanged messages with, most recent first; per person the counts each way, first and last contact, the user's median reply time, the language, twelve months of activity, handles, promises either way, recent messages; roles and notes are the user's to write (design 07) |
| Mail and Telegram on one timeline | done since the Telegram connector |

## M4 status

Design 10, "行动". Done when design 00's success standard is met whole: the author has used it for thirty days running, at least one reply it drafted was approved and went out, and the records replay the whole path.

| Task | State |
|---|---|
| Actions kept in the store and every step in the ledger: proposed, edited, approved, declined, expired, handed to a connector, executed, failed, unknown | done |
| The approval endpoint: local only, version and hash and a page nonce; editing makes a new version and voids an earlier approval; three days to expiry | done |
| The drafter: a reply in the user's voice from the conversation and the user's own messages, as version one of a pending action, on request from a message | done; quality to be judged in use |
| The approval panel: evidence, why, an editable draft, "Approve and send", a decline with a reason; the Today page shows the first five | done |
| Connectors pull approved actions, check them again, send once and report; the sent message flows back as an outbound item in the answered thread; an unknown outcome is confirmed by sync; a silent connector becomes unknown after ten minutes | done; to be exercised on a real reply |
| Withdrawing an approval before the connector takes it | done |
| Edited drafts and decline reasons as signal (design 07) | recorded as versions and records; not yet used |
| The conversation page: a bounded plan of at most eight model calls, one tool per step (search, thread, person, time, draft), every step on record and openable from the answer, cites checked against what was shown, a proposed draft shown as the same card the Approvals page uses | done; quality to be judged in use |
| The Records page shows every action step and every run, each run openable into its steps | done |

## Interface v0.4 status

Design 06 v0.4: one daemon, any device with a browser. Decided after M4, when the
first month of use began and the test page was not something to hand a phone.

| Task | State |
|---|---|
| Design 02, 03, 06, 08, 09 amended: pairing replaces the start-up token, approvals accept paired devices, the network boundary is the private network's, the shell shows the page in a window | done |
| Device pairing: a code made on loopback, one use, five minutes; a hashed secret per device in the store; a cookie the device carries; revocation; the test for design 02's invariant 11 | done |
| The page rebuilt with Svelte and Vite, phone first, two languages following the system, the eight screens and the pairing screen; built output committed and compiled into the core | done; to be judged in use |
| The menu bar shell opens the page in a web view window | done |
| `service install --bind` and `serve --bind` for a private network address | done |
| Design 06 v0.5: four faces, Today, Approvals, Chats, Ask; the timeline and the records behind "more" | done |
| Chats: a list of parties with each one's last word, and per party a conversation (theirs left, yours right, mail and chat together, group messages marked, paging back), the facts and the promises above it, and a reply drafted as an approval card in the stream | done; to be judged in use |
| Design 06 v0.6: a group or channel is one party; its members are not listed unless there is a one-to-one exchange; a person's conversation and statistics count only what passed one to one | done |
| Agents as parties in Chats (design 06 v0.5) | designed; waits for the first agent |
| Accounts from Settings: add a mailbox (checked first), sign in to Telegram in three steps, disconnect; the connectors restart with the new list; loopback only, design 02 invariant 12 with its test | done; the Telegram sign-in to be exercised with a real account |
| Models from Settings: pinned catalog (repository, commit, every file's size and SHA-256), disk check, resumable download with progress, each file verified before use, the model side started when it completes | done; the download itself to be exercised on a machine without the models |
| The gateway configuration written from the catalog on first run instead of by hand | done |
| A release build carrying Genatrix's Telegram application credentials, compiled in | done; needs the pair set at build time |
| Backup, recovery code, export, delete, diagnostics on the Settings page (design 09) | not started |
| First-run wizard (design 09) | not started |

## Spike status

| Spike | Status | Blocked on |
|---|---|---|
| Sandbox | **pass**, [spikes/01-sandbox.md](spikes/01-sandbox.md) | |
| Local model | **pass with conditions**, [spikes/02-local-model.md](spikes/02-local-model.md) | |
| Telegram login | **pass**, [spikes/04-telegram.md](spikes/04-telegram.md) | |
| Gmail IMAP | **pass on function**, [spikes/03-gmail-imap.md](spikes/03-gmail-imap.md); the speed threshold is proposed for revision | a decision on the design 10 threshold |

## What the spikes need

Development machine prerequisites: an Apple Silicon Mac with the Xcode Metal toolchain installed (`xcodebuild -downloadComponent MetalToolchain`) and enough free disk for model weights and build artifacts.

1. **Telegram application credentials.** Log in at my.telegram.org with your phone number, create an application, and note the `api_id` and `api_hash`. This is the pair design doc 05 describes as Genatrix's own; for now it is only used by the spike.
2. **An IMAP test account.** Ideally your own Gmail with two-step verification enabled and an app password generated. The password can be revoked afterwards. With it:

   ```sh
   genatrix account --add you@gmail.com     # asks for the app password, checks it, keeps it in the keychain
   genatrix serve                            # or: genatrix service install
   ```

   The password lives in the login keychain under "Genatrix mail" and nowhere else. `GENATRIX_IMAP_PASSWORD` in the environment overrides it, for development. `genatrix account --forget you@gmail.com` removes both the account and the password; what was fetched stays.

Never paste credentials into the chat. Put them in a file outside the repository, readable only by your user, for example `~/.config/genatrix-dev/secrets.toml`. Spike scripts read from there and never log them.

```toml
[telegram]
api_id = 0
api_hash = ""
phone = ""

[imap]
host = "imap.gmail.com"
user = ""
password = ""
```

## Repository layout

A Cargo workspace. Crate boundaries follow the design documents; each crate's root doc comment names the design it implements.

| Crate | Binary | Design | Responsibility |
|---|---|---|---|
| `genatrix-model` | | 01 | Entity types. Pure types plus serde, no storage dependency |
| `genatrix-keys` | | 08 | The master key and everything derived from it; later passphrase wrapping and the recovery key |
| `genatrix-store` | | 01, 08 | SQLCipher persistence, migrations, full-text and vector search, export and import |
| `genatrix-ledger` | | 02, 03, 08 | Append-only ledger: egress, run, and action records |
| `genatrix-gate` | | 02 | Content patterns, the rule file, redaction, the egress gate |
| `genatrix-agent` | | 03 | Envelopes, tool protocols, output collars, actions, the run context |
| `genatrix-profile` | | 07 | Relationships, facts, commitments, style; the least-privilege memory writer |
| `genatrix-llm` | `genatrix-llm` | 02, 04 | Egress tickets, the cloud policy, the model registry, the gateway binary |
| `genatrix-infer` | `genatrix-infer` | 04 | Local inference process, runs inside the sandbox with no network |
| `genatrix-connector` | | 05 | Connector protocol, account capabilities, IPC |
| `genatrix-connector-imap` | `genatrix-imap` | 05 | Mail connector (IMAP + SMTP) |
| `genatrix-connector-telegram` | `genatrix-telegram` | 05 | Telegram connector (user-account protocol) |
| `genatrix-daemon` | `genatrix` | 02, 03, 06, 09 | Assembles the layers, carries out model calls, runs pipelines; will serve the local web UI and the approval endpoint |

Dependencies point downward only: daemon → agent → gate → ledger/store → model, with `keys` at the bottom beside `model`. The gate also depends on `llm`, because the ticket type is part of the model layer's protocol: the gate mints what the gateway checks. Connectors depend only on `model` and their own protocol crate.

`vendor/crabllm/` holds a pinned copy of crabllm with provenance and patches recorded in `vendor/crabllm/GENATRIX-VENDOR.md`. It is outside the workspace (`exclude = ["vendor"]`) and reached by path dependencies. Its Swift build output under `mlx/.build/` is not committed and is rebuilt on a clean checkout, which takes several minutes.

The frontend (design 06) and the menu bar shell (design 09) are added when their milestone begins.

License: MIT OR Apache-2.0, following the ESSE convention.

## Working rules

- Each spike produces one result file under `docs/plan/spikes/` describing what was done, the measurements, the verdict, and the impact on the design. Spike code does not enter the repository.
- When design and code disagree, the design document changes first, then the code.
- Every invariant listed in design doc 02 gets a test in the same commit that introduces the code it constrains.
