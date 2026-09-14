# Kickoff

The eleven design documents in `docs/design/` are final. This page is the bridge from design to code: what happens first, who does it, and what it needs.

Design documents are written in Chinese for discussion with the author. Everything else in this repository, including this directory, code comments, and commit messages, is in English.

## Order of work

1. **Four technical spikes**, one day each, with pass thresholds defined in design doc 10. If any spike fails, the design changes before any code is written.
2. **Phase 0**: this machine, model selection, repository skeleton, gateway, storage. No real data enters the system during Phase 0.
3. **M1 · Collect**: the IMAP connector and the timeline. Product code starts here.

## Spike status

| Spike | Status | Blocked on |
|---|---|---|
| Sandbox | see [spikes/01-sandbox.md](spikes/01-sandbox.md) | nothing |
| Local model | not started | nothing |
| Telegram login | not started | Telegram application credentials |
| Gmail IMAP | not started | a mailbox with an app password |

## What the spikes need

Development machine prerequisites: an Apple Silicon Mac with the Xcode Metal toolchain installed (`xcodebuild -downloadComponent MetalToolchain`) and enough free disk for model weights and build artifacts.

1. **Telegram application credentials.** Log in at my.telegram.org with your phone number, create an application, and note the `api_id` and `api_hash`. This is the pair design doc 05 describes as Genatrix's own; for now it is only used by the spike.
2. **An IMAP test account.** Ideally your own Gmail with two-step verification enabled and an app password generated. The password can be revoked after the spike.

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
| `genatrix-store` | | 01, 08 | SQLCipher persistence, migrations, full-text and vector search, export and import |
| `genatrix-ledger` | | 02, 03, 08 | Append-only ledger: egress, run, and action records |
| `genatrix-gate` | | 02 | Sensitivity rules, classification orchestration, redaction, the egress gate, egress tickets |
| `genatrix-agent` | | 03 | Task, Run, tools, tool protocols, pipelines, Action |
| `genatrix-profile` | | 07 | Relationships, facts, commitments, style; the least-privilege memory writer |
| `genatrix-llm` | `genatrix-llm` | 04 | Roles and the model registry; the gateway binary embedding crabllm |
| `genatrix-infer` | `genatrix-infer` | 04 | Local inference process, runs inside the sandbox with no network |
| `genatrix-connector` | | 05 | Connector protocol, account capabilities, IPC |
| `genatrix-connector-imap` | `genatrix-imap` | 05 | Mail connector (IMAP + SMTP) |
| `genatrix-connector-telegram` | `genatrix-telegram` | 05 | Telegram connector (user-account protocol) |
| `genatrix-daemon` | `genatrix` | 06, 09 | The core process: assembles the layers, serves the local web UI and the approval endpoint |

Dependencies point downward only: daemon → agent → gate → ledger/store → model. Connectors and the LLM crates depend only on `model` and their own protocol crates. CI rejects anything that violates this direction.

The frontend (design 06), the menu bar shell (design 09), and the vendored crabllm tree are added when their Phase 0 task begins.

License: MIT OR Apache-2.0, following the ESSE convention.

## Working rules

- Each spike produces one result file under `docs/plan/spikes/` describing what was done, the measurements, the verdict, and the impact on the design. Spike code does not enter the repository.
- When design and code disagree, the design document changes first, then the code.
- Every invariant listed in design doc 02 gets a test in the same commit that introduces the code it constrains.
