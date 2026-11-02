# Executors and dispatch

Design 13 v0.2 (rulings 3, 4 and 11 made; the rest implemented as recommended and awaiting review), with proposed invariants 22–27, the revision of invariants 11 and 12, and the new purpose `Delegate`. This page is the shape of the code as built, what each step proved, and what is left that needs the author at the machine.

## Status

E0 to E9 are written and tested with stand-ins for the executors and for GitHub. Nothing has yet driven a real Claude Code or Codex session, created the executor's system user, or pushed to GitHub: each needs the author's password, accounts or token. The checklist at the end is what remains.

## Shape

```
crates/dispatch            projects, roles, recipes (closed schemas), the built-ins, the step machine,
                           the permission scope, prompt envelopes; pure, no I/O
crates/exec                genatrix-exec: the executor host; a small ACP client, the ticket check,
                           the git work (mirror, workspace, commit, bundle); a fake ACP agent for tests
crates/connectors/github   genatrix-github: one-shot, sandboxed; pushes with git, opens pull requests
crates/daemon/src/executors  config, definitions, drafts, the runner, the GitHub runner, the terminal,
                           model-filled drafts
crates/daemon/src/web/work.rs  the page's API;  web/src/pages/Work.svelte  the page
```

`dispatch` depends on nothing of ours. Both the core and the host use its scope check.

`exec` depends on `dispatch`, `keys` and `llm` (for the ticket). It does not link `store` or `gate`.

`connector-github` depends on nothing of ours; it is given one request on standard input and writes one reply.

### Where the build differs from the first plan

- **No `agent-client-protocol` crate.** A client needs initialize, session/new, one session/prompt, the agent's permission requests and its updates: `crates/exec/src/acp.rs`, a few hundred lines, protocol version 1. The crate's API has moved between minor versions; this does not. Spike 05 checks the shapes against the real adapters.
- **The host talks over pipes, not a socket.** The core starts `genatrix-exec` per step and owns its standard input and output, so no token is needed and nothing else can connect. The host runs as the user; it starts the executor through `sudo -n -H -u _genatrix_exec`. Its delegation key is not the gateway's: the gate makes a fresh one per run of the core.
- **Executor output is kept in dispatch tables, not as items** (`dispatch`, `dispatch_step`, `dispatch_event`, migration 16). It is not the user's life; it would pollute the timeline, the digest and search. Design 13 v0.2 says so.
- **The core runs the git work**, as the executor's user, through `sudo`, rather than routing it through the host.
- **A dispatch action is executed when its work starts.** Work runs longer than the core's ten-minute report window; the dispatch record carries the rest.
- **GitHub is a one-shot process per action**, not a long-running connector: there is nothing to sync yet, and a process that lives for one action holds the token for one action. No issue ingestion and no `CommentOnIssue` in this version.
- **The scope allows a declared command with plain arguments**, and read-only git commands; the host commits what an executor leaves, so `git commit` is never an executor's to run.
- **Executor work has its own five-second loop** in the core, independent of the local model being ready.

## Steps

| Step | What | Proved by |
|---|---|---|
| E0 | Local identity: the key file, `genatrix open`, the shell's header and cookie; design 02 v0.3 and 06 v0.11 | `web/access.rs`, `web/local.rs` tests: loopback without the key is refused everywhere; the key from elsewhere is refused; a link works once |
| E1 | Spike 05 | Not done. See the checklist |
| E2 | `crates/dispatch` | 22 tests: schemas, the loop bound, inputs from earlier steps, the scope (paths, `..`, links, `.git`, commands with `;` `&&` `$()` `>`), envelopes that cannot be closed from inside (invariants 25, 26) |
| E3 | `crates/exec` | the real host binary driving the fake ACP agent: permissions answered by the scope from both ends; no session without a ticket for exactly these bytes, this executor, this key, the delegate purpose, a level that may leave (invariant 22); stop and time limit; a not-signed-in executor names the login command |
| E4 | `genatrix exec setup`, `status`, `login`; mirror, workspace, bundle | the git round trip; a bundle from elsewhere refused; the executor started as its own user (invariant 23); setup's commands and its one narrow sudo rule |
| E5 | Core: store, `Delegate` and the gate's delegation path, `Dispatch`, the runner | gate tests (matrix, refusals recorded, ticket bound to bytes); the whole recipe end to end with a scripted host; a recipe changed after approval does not change the work (26); executor words start nothing and are not items (27); stop; drafts refuse what could not be sent |
| E6 | `genatrix-github`, `genatrix github login` | only the approved, still-filed commit is pushed, checked before the network (24); the sandbox profile |
| E7 | The Work page; approval cards with facts and the public switch | type check and build; access: a paired phone may draft and watch, only this machine files a project |
| E8 | `implement-and-review` end to end | with the scripted host, on a real git repository. `implement` with the real Claude Code, through to a pushed branch and a merged pull request on a side repository; Codex and the review loop not yet run for real |
| E9 | Drafts filled in by the local model (`Plan`) | the form is filled; nothing is drafted until the user presses draft |

## For the author, at the machine

In this order. Each line is one command or one look.

1. `cargo build --workspace` (the Metal Toolchain is missing for `genatrix-infer`: `xcodebuild -downloadComponent MetalToolchain`).
2. Spike 05, handshake only, as yourself, in an empty directory: run `npx -y @zed-industries/claude-agent-acp` and `npx -y @zed-industries/codex-acp`, send `initialize` and `session/new`, and check the shapes against `crates/exec/src/acp.rs`. This was not run here: fetching and running the adapters needs your approval. If the package names have changed, fix them in `executors.toml`.
3. `brew install node` if Node only exists under `~/.nvm`: the executor's user cannot see your home.
4. `genatrix exec setup` (asks for your password once; `--print` shows everything first), then `genatrix exec login claude-code` and `genatrix exec login codex`, then `genatrix exec status` until every line says ok.
5. `genatrix github login` with a fine-grained token: Contents and Pull requests, read and write, on the project repositories only.
6. `genatrix project add` a small side repository first, with `delegate_personal` left off.
7. On the Work page: a small task with `implement`, then `implement-and-review`. Watch the steps and what the executors say; read the diff; approve the push, then the pull request.
8. Check the egress records page: every text sent to an executor is there, with its target.

## Spike 05 (partly run)

First real run, `@zed-industries/claude-agent-acp` 0.23.1 as `_genatrix_exec`: `initialize` and `session/new` complete with `crates/exec/src/acp.rs` as written. Without a login, the adapter opens the session and refuses at `session/prompt` with `-32000`; the host now names `genatrix exec login` there too. The login commands fetch `@anthropic-ai/claude-code` and `@openai/codex` through `npx`, because a `claude` or `codex` installed in the author's home is not visible to the executor's user.

One day, no code kept.

| Question | Pass | If not |
|---|---|---|
| Does `crates/exec/src/acp.rs` complete a turn with both adapters, the client declaring no `fs` and no `terminal`? | both finish a small edit | fix the shapes; drive the one that works first |
| Do edits and commands arrive as `session/request_permission` with paths and the command line? | yes for both | the scope falls back to the system user alone; design 13 says so |
| Does a refused request leave the session usable? | the executor carries on or says why | treat a refusal as the end of the step |
| Does `sudo -n -u _genatrix_exec` start the adapters with Node found? | `genatrix exec status` all ok | adjust `path` in `executors.toml` |
| Does Claude Code ask before it edits, in the mode the adapter starts it in? | yes | set its permission mode for the executor's user in its own settings |
