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

- [Design documents](docs/design/), written in Chinese. Start with
  [00 Vision and Principles](docs/design/00-vision.md).
- [Implementation plan](docs/plan/) and spike results, in English.

Runs on Apple Silicon Macs. 16 GB of memory or more.

## License

MIT OR Apache-2.0.
