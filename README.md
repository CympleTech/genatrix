# Genatrix

An AI that runs only on your device, understands your digital life, and acts
only with your approval.

Your mail and messages are downloaded to your own computer, encrypted with
your own key, and read there by a model running on the same machine. Nothing
leaves without you knowing. Over time it comes to know who you talk to, what
you owe people, and how you write, and it starts drafting on your behalf.

## What it does

**Brings everything into one timeline.** Mail and chat land in a single
stream, not one app per source. Search once and find both. Your day did not
happen in separate applications and neither should your record of it.

**Reads it on your machine.** A local model classifies, summarizes, and
searches. Sensitive work never leaves the device, and nothing does unless you
switch the cloud on.

**Sorts by sensitivity, in three levels.** Public, personal, secret. Rules you
can read and edit set the floor; the model may raise it; only you may lower
it. Verification codes, card numbers, identity numbers and passwords are
recognised with their checksums, so an order number is not mistaken for a
credit card.

**Never sends anything you have not seen.** Everything bound for a cloud model
passes one gate, which redacts names and secrets, writes down the exact bytes
before they go, and issues a single-use permission tied to those bytes. A
second process checks that permission independently and refuses anything it
cannot account for.

**Keeps a ledger you can read.** Every model call, every run, every proposed
action, in an append-only chain. Open it and the first line answers the
question that matters: how many bytes left this device, and what were they.

**Asks before acting.** A reply, a calendar entry, a note to memory: the agent
proposes, you approve. Approval binds to the draft you read, so editing it
voids the approval rather than quietly sending different words. Approvals are
single-use and expire, so Monday's cannot fire on Friday.

**Shows its reasoning before its words.** Every summary point cites the
messages it came from. A claim with no source is marked as unsupported rather
than mixed in.

**Treats every message as untrusted input.** A mail that says "ignore your
instructions" is put where instructions are not read, the model's output is
constrained to shapes that cannot carry an attack, and nothing reaches the
outside world without a person. The worst a successful injection achieves is a
wrong summary.

**Lets you leave.** Export is one button and produces plain JSON plus your
files. The point of owning your data is being able to take it somewhere else.

## What it does not do

No accounts, no servers, no telemetry. No selling you a subscription to your
own mail. It is one person's data on one person's computer, and everything
else follows from that.

## Status

Early. The foundations are built and tested: storage, the ledger, the
sensitivity rules, redaction, the egress gate, the local inference process,
the gateway, and the agent layer. The mail and chat connectors come next,
followed by the interface.

There is no installer and no application yet. What follows is how to run the
pieces that exist, from a source checkout. When it ships, none of this will be
necessary: design 09 describes a signed app, seven screens, and no terminal.

- [Design documents](docs/design/), written in Chinese. Start with
  [00 Vision and Principles](docs/design/00-vision.md).
- [Implementation plan](docs/plan/) and spike results, in English.

## Requirements

An Apple Silicon Mac with 16 GB of memory or more, running one of the last two
macOS releases. Apple's Metal toolchain, which the local inference build needs:

```sh
xcodebuild -downloadComponent MetalToolchain
```

Around 20 GB free: model weights, and a first build that compiles SQLCipher,
OpenSSL and the MLX Swift package. That first build takes several minutes;
later ones are quick.

## Running it

**Build.**

```sh
cargo build --release -p genatrix-infer -p genatrix-llm
```

**Get a model.** Any MLX model directory works; this is the one the local
model spike measured, about 4.3 GB.

```sh
mkdir -p ~/.genatrix-dev/run
uvx --from huggingface_hub hf download mlx-community/Qwen3-8B-4bit \
  --local-dir ~/.genatrix-dev/models/qwen3-8b-4bit
```

(`uvx` runs it without installing anything. With the Hugging Face CLI already
on your machine, `hf download` on its own does the same.)

**Write a gateway configuration** at `~/.genatrix-dev/gateway.toml`. Keep the
socket paths short: macOS caps a Unix socket path at 104 bytes.

```toml
socket = "/Users/you/.genatrix-dev/run/gateway.sock"

[[models]]
name = "local"                     # what callers ask for, and what a ticket names
model = "qwen3-8b-4bit"            # what the inference process is serving
context_length = 32768
purposes = ["classify", "extract", "embed", "identity_suggestion",
            "summarize", "draft", "translate", "search_rewrite", "plan"]
endpoint = { kind = "local_socket", path = "/Users/you/.genatrix-dev/run/infer.sock" }
```

Check it before starting anything. A configuration that would send
classification to a cloud model, or that leaves a purpose with no local model
to fall back to, is refused here rather than at the first request.

```sh
./target/release/genatrix-llm --config ~/.genatrix-dev/gateway.toml --check
```

**Start the inference process**, inside a sandbox that removes its network
access. It prints the profile it wants; hand that to `sandbox-exec`.

Run these from the checkout. `$DEV` is just shorthand for the data directory.

```sh
DEV=~/.genatrix-dev

./target/release/genatrix-infer \
  --model-dir $DEV/models/qwen3-8b-4bit --model-name qwen3-8b-4bit \
  --socket $DEV/run/infer.sock --print-sandbox-profile > $DEV/infer.sb

sandbox-exec -f $DEV/infer.sb ./target/release/genatrix-infer \
  --model-dir $DEV/models/qwen3-8b-4bit --model-name qwen3-8b-4bit \
  --socket $DEV/run/infer.sock
```

It loads the model in a few seconds and logs `listening`. From inside that
sandbox it can reach its own socket and nothing else: not the network, not
another program's socket, and neither can anything it starts.

**Start the gateway**, in another terminal. It shares a secret with whatever
mints tickets, read from the environment so it never appears in the process
list. The gateway refuses every request without it, so it will not start
without one either.

```sh
export GENATRIX_TICKET_KEY=$(openssl rand -hex 32)
./target/release/genatrix-llm --config ~/.genatrix-dev/gateway.toml
```

**Try it.** The gateway answers only to a ticket that covers these exact
bytes, so a request without one is refused:

```sh
curl --unix-socket ~/.genatrix-dev/run/gateway.sock \
  http://localhost/v1/chat/completions \
  -H 'content-type: application/json' \
  -d '{"model":"local","messages":[{"role":"user","content":"hello"}]}'
# {"error":{"type":"ticket_rejected", ...}}   401
```

To mint one by hand, there is a development example. In the finished system
only the egress gate mints tickets, after it has judged the content, redacted
it, and written the ledger entry.

```sh
cargo build --release -p genatrix-llm --example mint_ticket

BODY='{"model":"local","max_tokens":64,"messages":[{"role":"user","content":"hello"}]}'
TICKET=$(printf '%s' "$BODY" | ./target/release/examples/mint_ticket \
  --target local --purpose summarize --level personal)

curl --unix-socket ~/.genatrix-dev/run/gateway.sock \
  http://localhost/v1/chat/completions \
  -H 'content-type: application/json' \
  -H "x-genatrix-ticket: $TICKET" \
  -d "$BODY"
```

Change one byte of the body and the same ticket stops working. Send the same
ticket twice and the second is refused. Ask a cloud model for something marked
secret and it never leaves.

## Configuring it

**`gateway.toml`** lists the models and where each one runs. `local_socket` is
the sandboxed process on this machine; `openai` and `anthropic` are cloud
endpoints, which this build accepts in configuration but has no transport for:
the cloud opens in phase two, and an untested path out of the machine is the
one thing this gateway exists to prevent.

**`GENATRIX_TICKET_KEY`** is the secret the gate and the gateway share, 64 hex
characters. Generate a fresh one per run. It authenticates permission to send,
not data at rest.

**The sensitivity rules** live in
[`crates/gate/rules/default.toml`](crates/gate/rules/default.toml), and that
file is worth reading: it is where "what counts as private" is written down in
a form you can argue with. Rules that say *secret* escalate and nothing can
take that back; rules that say *public* or *personal* classify, and later ones
override earlier ones. An item no rule matches is personal. Content patterns
are built in, because a card number needs a checksum and a regex you can break
by accident is a poor place to keep a guarantee, but every one of them can be
switched off in that file.

**Nothing configures the cloud on.** There is no switch yet, and when there is
one it will be off by default.

## Development

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

All three should be clean.

## License

MIT OR Apache-2.0.
