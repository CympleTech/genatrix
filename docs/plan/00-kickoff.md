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
| Model selection against a real evaluation set: the author's mailbox, 100 sampled judgements | not started; the first pass over the real mailbox is the input |
| Chunking, embedder role, `sqlite-vec`, vector search on the page | not started |
| Summarizer role; summary shown per item | not started |
| Records page backed by the ledger for every model call | partly: the page reads the ledger; the item count of model calls will grow with the pipelines |
| Telegram connector (non-blocking; due by the end of M3) | not started; the login spike needs application credentials |

## Spike status

| Spike | Status | Blocked on |
|---|---|---|
| Sandbox | **pass**, [spikes/01-sandbox.md](spikes/01-sandbox.md) | |
| Local model | **pass with conditions**, [spikes/02-local-model.md](spikes/02-local-model.md) | |
| Telegram login | not started | Telegram application credentials |
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
