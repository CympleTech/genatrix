# Functional agents

Design 11 (with 02 invariants 13–21, 06 v0.7, 08 v0.3). This page is the order of work, the shape of the code, and what each step must prove before the next begins.

Design 10 places this after the first outside user. The author chose to start the framework now; the phase-one gates (the 100-item review, the fourteen-day digest, thirty days of use, the first real approved reply) still stand and are not replaced by anything here.

## Shape

```
wit/genatrix-agent.wit     the one contract: the world an agent is built against
crates/host                genatrix-host: manifest, package, wasmtime runner, limits, the Doors trait
crates/daemon              the doors implemented against store, gate and actions; triggers; pages
agents/                    a separate Cargo workspace built for wasm32-wasip2
  sdk/                     genatrix-agent-sdk: wit-bindgen guest bindings and small helpers
  hello/                   the smallest agent; used by the host's tests
  nz-tax/                  the first agent; taxcore and taxrules move here from ../finance
  travel/                  the second agent
```

`crates/host` sits beside `agent` in the layering: it depends on `model` and nothing that holds data. Everything that touches data comes in through the `Doors` trait, which the daemon implements. The host cannot reach the store, the gate or the network because it does not link them.

`agents/` is outside the root workspace, like `vendor/`: it builds for another target and its crates are guest code. The host's tests use prebuilt components committed under `crates/host/tests/fixtures/`, rebuilt by a script, so `cargo test --workspace` does not need the wasm target.

### Package

One `.wasm` component. Custom sections carry the rest:

| Section | Content |
|---|---|
| `genatrix:manifest` | TOML, validated against a closed schema (`deny_unknown_fields`) |
| `genatrix:prompts` | the prompts, by name |
| `genatrix:fixtures` | optional test inputs and expected proposals |

The version is the SHA-256 of the file. The manifest is read without instantiating anything.

### Runner

- wasmtime with the component model. Fuel for instructions, `StoreLimits` for memory, epoch interruption for wall time; each run a fresh `Store` and instance.
- WASI is linked interface by interface, never whole: clocks (the wall clock is the run's `now`, fixed for the run), random (seeded per run and recorded), stdout and stderr discarded (the `log` door is the record), environment empty, filesystem with no preopens. `wasi:sockets` and `wasi:http` are not linked, so a component that imports them fails to instantiate. That failure is the test for invariant 13.
- Guest exports: `on-items(ids)`, `on-message(text)`, `on-schedule(name)`, `apply(kind, payload)`.

### Doors

The WIT imports, each backed by one method on `Doors`:

| Door | Checked where | Invariant |
|---|---|---|
| `items.query(filter)` | the daemon intersects the filter with the manifest's read scope before the query runs | 14 |
| `items.get(id)` | outside the scope reads as absent | 14 |
| `blobs.text(id)` | only if the manifest declares it; the item must be in scope | 14 |
| `model.call(purpose, messages)` | purpose in the manifest; level is the run's taint, not the caller's word; initiator `Installed { agent, version, run }` | 1, 13, 20 |
| `space.execute / space.query` | the agent's own SQLCipher file; an authorizer refuses ATTACH, DETACH, extension loading and settings pragmas; `max_page_count` holds the quota | 15 |
| `actions.propose(kind, card, target?)` | kind in the manifest; outward targets checked against the manifest's target rule; the daily cap | 16, 21 |
| `now`, `log` | | |

The run's taint starts at the space's level and rises with every item and blob text read through a door. The host keeps it; the daemon's doors report the level of what they returned.

### Actions

- `Effect::Agent { agent, kind, card }` for effects inside the agent's own space. Approved, it is executed by calling the agent's `apply` export with the card's payload and a space handle; nothing else is linked for that call.
- Outward effects reuse `SendMail`, `SendMessage`, `CreateEvent`, with the target rule applied before the action is stored.
- The card is data: a title, labelled fields (text, money, date), evidence item ids. The page renders it with one fixed component.

## Order of work

Each step ends with the checks clean and the invariants it touches tested.

| Step | What | Proves |
|---|---|---|
| A1 | WIT world; manifest and package reading; the runner with limits and curated WASI; `Doors` trait with an in-memory fake; `agents/sdk` and `agents/hello`; fixtures committed | invariants 13 and 19 against real components |
| A2 | Agent registry and approved versions in the store; space files with the derived key and the authorizer; the daemon's doors for items, blobs, model and space; the run's taint; run records | 14, 15, 17, 20 |
| A3 | `Effect::Agent` and `apply`; outward targets; the daily cap; fixed-template cards on the Approvals page | 16, 21 |
| A4 | Lifecycle: install from a file on loopback (the manifest in words, risk tiers), dry run on a read-only snapshot, triggers (new item in scope, a message, a schedule), pause, uninstall with export; the agent as a party in Chats; its ledger page | the whole path with `hello` |
| A5 | PDF text for `blobs.text`, extracted inside a wasm sandbox of its own (an untrusted parser does not run in the core's address space) | |
| A6 | NZ tax: taxcore and taxrules moved into `agents/nz-tax`, the float rates made rational, taxstore's schema and triggers in the space, extraction by the local model, validation by taxcore, `RecordEntry` cards, GST101 and IR3 on request | one real GST period, every entry approved |
| A7 | Upgrades: a new version as an approval with the manifest diff, a replay of past inputs through both versions, fixtures; rollback | 17 |
| A8 | Management devices and the passphrase (design 06 v0.7); remote high-privilege operations | 11, 12, 18 |
| A9 | Calendar connector; the travel agent | the interface holds for a second, different agent |
| A10 | The developer guide, `genatrix agent test` (replay against fixtures or a snapshot, nothing executed), then open | |

## Status

| Step | State |
|---|---|
| A1 | done: the WIT world; manifest (closed schema, ceilings) and single-file package; the runner with fuel, memory and wall-time limits, a fixed clock, seeded randomness, WASI linked interface by interface without sockets; the `Doors` trait; `agents/sdk`, `agents/hello`, and two probes; fifteen sandbox tests against the built components, covering invariants 13, 19, 20 and the host's half of 14 |
| A2 | done: `agent`, `agent_version`, `agent_run` tables; each space its own SQLCipher file keyed by `space_key(agent_id)`, with attach limit zero, an authorizer against attach, pragmas, virtual tables and extensions, one statement per call, a page ceiling, and a deadline while a statement runs; the daemon's doors against the store (read scope as an `ItemQuery` the agent's filter only narrows), the gate (`Initiator::Installed`, local unless the agent's own cloud switch is on), and the space; packages kept per version and refused unless their hash is approved; each run recorded in the store and, by reference, in the ledger; `genatrix agent install/list/ask/pause/resume/runs` on this machine. Tests for invariants 14, 15, 17, 20 and ruling 12. Attachment text and proposals answer "not available yet" until A5 and A3 |

## Decisions taken here

- wasmtime 49, the component model, guests on `wasm32-wasip2`. Rust is the first guest language because the tax core is Rust; the WIT file is the contract for any other.
- Space as SQL rather than key-value: the tax ledger is relational and its tamper resistance is SQLite triggers, which carry over unchanged.
- Until A8, every high-privilege operation stays loopback only, stricter than design 06 v0.7 allows.
