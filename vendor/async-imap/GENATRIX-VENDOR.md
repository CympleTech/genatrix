# Vendored async-imap

Upstream: https://github.com/async-email/async-imap
Version:  0.11.3 (crates.io release, copied from the local registry)
License:  MIT OR Apache-2.0 (see LICENSE-MIT and LICENSE-APACHE in this directory)

Why vendored: `imap-proto` parses Gmail's `X-GM-THRID` fetch attribute, but
`async-imap` 0.11 exposes an accessor only for `X-GM-MSGID`. Without the thread
id the mail connector falls back to the `References` chain for conversations,
which misses what Gmail itself groups. One accessor is too small a change to
wait for a release on, so Genatrix carries the crate with the patch applied,
through `[patch.crates-io]` in the workspace `Cargo.toml`.

Rules (design doc 05, same as for crabllm):
- Local changes are kept as patch files under `patches/` and applied on top of
  the upstream tree, so they can be reviewed against upstream and offered back.
- Return to the crates.io release, and drop this directory, once upstream ships
  the accessor.

Patches applied:

- `patches/0001-fetch-gmail-thread-id.patch` — `Fetch::gmail_thr_id()`, the
  twin of the existing `gmail_msg_id()`. Worth offering upstream as is.

Not copied: the upstream tests and examples directories and `Cargo.lock`; the
crate is consumed as a library and its own tests are upstream's to run.
