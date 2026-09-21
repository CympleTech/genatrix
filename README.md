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

Milestone one, collecting mail, is built and in its soak. A Gmail account
signs in once, its history arrives newest first, new mail appears within
seconds, and it all runs from login as a background program with a menu bar
icon. The connector runs in its own sandboxed process; the core starts it and
stores what it brings. Storage, the ledger, the sensitivity rules, redaction,
the egress gate, the local inference process, the gateway, the agent layer
and a local web interface are in place underneath. With synthetic items in
the store, the classification pipeline judges them on this machine and the
records page reports that nothing left the device. Understanding the mail,
milestone two, comes next; Telegram alongside it.

There is no installer yet. What follows is how to run the pieces that exist,
from a source checkout. When it ships, none of this will be necessary: design
09 describes a signed app, seven screens, and no terminal.

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

This is the developer's path: a checkout, a terminal, and the pieces started
by hand where the finished application (design 09) will do it for you. The
end state is the same: a background program that starts at login, a menu bar
icon, and a local page with your mail on it.

**Build.**

```sh
cargo build --release --workspace
swift build -c release --package-path apps/menubar
cp apps/menubar/.build/release/genatrix-menubar target/release/
```

The first build compiles SQLCipher, OpenSSL and the MLX Swift package and
takes several minutes; later ones are quick. It produces the core
(`genatrix`), the mail connector it starts (`genatrix-imap`), the model
gateway (`genatrix-llm`) and the inference process (`genatrix-infer`). The
Swift build produces the menu bar shell (`genatrix-menubar`). The connector
and the shell have to sit beside the core: the core starts what it finds
next to itself.

**Choose a data directory.** Everything Genatrix keeps lives under one
directory. The default is `~/Library/Application Support/Genatrix`, and there
the master key lives in the login keychain, so a copy of the directory is
ciphertext without this account. Any other directory, named with
`--data-dir`, is a development one: the master key stays in a file beside the
data, and the core says so when it opens it. The examples below use a
development directory; drop `--data-dir` for the real one.

```sh
DEV=~/.genatrix-dev
mkdir -p $DEV/run
```

**Write a gateway configuration.** The core insists on this file before it
creates anything, because it has to agree with the gateway about which
models exist and where each one runs, even on a day no model is called.
Socket paths have to be absolute, and macOS caps them at 104 bytes, so let
the shell fill in your home directory rather than typing it:

```sh
cat > $DEV/gateway.toml <<EOF
socket = "$HOME/.genatrix-dev/run/gateway.sock"

[[models]]
name = "local"                 # what callers ask for, and what a ticket names
model = "qwen3-8b-4bit"        # what the inference process is serving
context_length = 32768
purposes = ["classify", "extract", "embed", "identity_suggestion",
            "summarize", "draft", "translate", "search_rewrite", "plan"]
endpoint = { kind = "local_socket", path = "$HOME/.genatrix-dev/run/infer.sock" }
EOF
```

**Initialise, and add your mailbox.**

```sh
./target/release/genatrix --data-dir $DEV init
./target/release/genatrix --data-dir $DEV account --add you@gmail.com
```

`init` creates the keys, the two databases and the rules file. `account
--add` works out the server from the address for the common providers
(`--imap-host` for the others), asks for the password without echoing it,
tries it against the server, and only then keeps it, in the login keychain
under "Genatrix mail". On Gmail that is an app password: turn on two-step
verification, then create one under App passwords. A refused password is not
stored. Run the same command again to sign in again after changing the
password; `account --forget you@gmail.com` removes the account and its
password and keeps what was fetched.

**Run it.** For a look, in a terminal:

```sh
./target/release/genatrix --data-dir $DEV serve
# Genatrix is at http://127.0.0.1:7717
```

For good, as the background program design 09 describes:

```sh
./target/release/genatrix --data-dir $DEV service install
```

That writes a launch agent for your user
(`~/Library/LaunchAgents/xyz.dpt.genatrix.plist`) and starts it. From then on
Genatrix starts when you log in. With the shell beside the core the agent
runs the shell, which puts an icon in the menu bar and runs the core behind
it; without the shell it runs `serve` alone. The icon has four looks, up to
date, syncing, needs you, error; clicking it lists each account with what it
is doing, opens the page, or quits. Quitting from the menu stops the core
too and stays stopped until the next login; a crash is restarted. The log is
at `$DEV/logs/genatrix.log`. `service status` says whether it is installed,
`service uninstall` stops and removes it. After a rebuild, run `service
install` again: it replaces the agent and restarts.

What happens once it runs: the core starts the mail connector in its own
process under a macOS sandbox that allows outbound connections only on the
ports the accounts were granted (993 for IMAP, 587 for submission), name
resolution, and the core's own socket; the connector cannot read the data
directory or the keychain files, and cannot start another program. The
profile it runs under is written to `$DEV/run/imap.sb` for you to read. The
connector walks the mailbox newest first, so this week's mail is on the page
within minutes, and takes new mail as it arrives: within seconds on servers
with IDLE, within a minute elsewhere. Progress and state per account are on
the page and in the menu.

**Open the interface.** A timeline you can search and filter, each item
expanding to show its full text and every judgement made about it, with who
made it and when; a records page that opens with how many bytes have left
the device and lists every model call; and a line per account at the top.

To look at it from a phone, bind somewhere else. That needs an access token,
which is generated per run and printed inside the link:

```sh
./target/release/genatrix --data-dir $DEV serve --bind 0.0.0.0
# Genatrix is at http://192.168.1.20:7717/?token=6a5915554214...
```

Loopback needs no token, because anyone who can reach it already has an
account on the machine. Any other address does, because the page has no login
and everything in it is your mail. It is still plain HTTP with one shared
secret: fine on a network you trust, not fine on one you do not. Pass
`--token` to keep a link working across restarts.

**Read it all again.** When normalization improves, `genatrix reprocess`
derives every item again from its stored raw record, in place; nothing is
fetched.

### The model side

Sensitivity judgement, and everything after it, needs the local model. None
of this is required for collecting and searching mail.

**Get a model.** Any MLX model directory works; this is the one the local
model spike measured, about 4.3 GB.

```sh
uvx --from huggingface_hub hf download mlx-community/Qwen3-8B-4bit \
  --local-dir $DEV/models/qwen3-8b-4bit
```

(`uvx` runs it without installing anything. With the Hugging Face CLI already
on your machine, `hf download` on its own does the same.)

**Check the gateway configuration** before starting anything:

```sh
./target/release/genatrix-llm --config $DEV/gateway.toml --check
```

The check refuses a configuration that could not work, rather than letting it
fail later with something cryptic: a socket path that is relative, too long,
or in a directory that cannot be created; a purpose routed to a cloud model
that may never serve it; a purpose with no local model to fall back to.

**Start the inference process**, inside a sandbox that removes its network
access. It prints the profile it wants; hand that to `sandbox-exec`.

```sh
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

**Start the gateway**, in another terminal. It needs a secret, which it shares
with whatever mints tickets. It is read from the environment rather than the
command line so it never appears in the process list, and the gateway will not
start without one: with no key every request would be refused, so there would
be nothing to serve.

```sh
export GENATRIX_TICKET_KEY=$(openssl rand -hex 32)
./target/release/genatrix-llm --config $DEV/gateway.toml
```

**Try it.** The gateway answers only to a ticket that covers these exact
bytes, so a request without one is refused:

```sh
curl --unix-socket $DEV/run/gateway.sock \
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

curl --unix-socket $DEV/run/gateway.sock \
  http://localhost/v1/chat/completions \
  -H 'content-type: application/json' \
  -H "x-genatrix-ticket: $TICKET" \
  -d "$BODY"
```

Change one byte of the body and the same ticket stops working. Send the same
ticket twice and the second is refused. Ask a cloud model for something marked
secret and it never leaves.

**Judge what you collected.** With the gateway and the inference process
running, and the same key in this terminal:

```sh
export GENATRIX_TICKET_KEY=<the same key the gateway got>
./target/release/genatrix --data-dir $DEV classify    # judge them, on this machine
./target/release/genatrix --data-dir $DEV timeline
./target/release/genatrix --data-dir $DEV ledger
```

`classify` reports how many items the rules settled on their own and how many
needed the model. `ledger` opens with the line design 06 asks for:

```text
0 bytes have left this device.
```

`genatrix seed` puts synthetic mail and chat in, for working on the pipeline
before an account exists.

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
