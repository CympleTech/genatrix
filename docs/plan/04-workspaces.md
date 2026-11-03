# Workspaces

Design 14 v0.2, implemented as recommended and awaiting review, with proposed invariants 30–33. A workspace is a scope over the one base, not a second database. It names accounts and owns projects. It decides which accounts' items may reach which executors, and whose executor and GitHub sign-ins its dispatches use.

## Steps

| Step | What | State |
|---|---|---|
| W1 | The definition, membership, the isolation rule at draft and at run, per-workspace sign-ins, dispatch from a task, a home per space on the page (design 06 v0.14) | This change |
| W2 | Filter the timeline, the digest and search by workspace | Not started |
| L2 | Per-workspace "own content as written" (design 14, open question 4), tasklist → project, auto-drafted dispatches | Next, on top of W1 |

## W1 as built

```
crates/dispatch/src/workspace.rs   Workspace (closed schema), account keys, ownership, the delegation check; pure
crates/dispatch/src/project.rs     `workspace`, default `personal`
crates/dispatch/src/builtin.rs     the built-in `personal` workspace
crates/daemon/src/executors        filing checks (one workspace per account, the project's workspace exists,
                                   a workspace in use is not removed); the draft's source; the run's re-check;
                                   per-workspace GitHub token; per-workspace executor login
crates/exec/src/session.rs         reads `~/.genatrix/<workspace>/<executor>.env`, else `~/.genatrix/<executor>.env`
crates/daemon/src/web              workspaces in the work overview, on accounts, on task rows; draft from a task
crates/daemon/src/web/spaces.rs     /api/spaces, /api/spaces/{name}: a space's open tasks, work going on, record, make-up
web/                               Spaces (list and one home per space) in place of the Work list; WorkForm;
                                   "Dispatch" on a task; the workspace on accounts and task rows
```

- **Definition.** Kind `workspace`, filed like a project with `genatrix workspace add <file>`. The file looks like this:
  ```toml
  name = "dpt"
  title = "DPT"
  accounts = ["lark:ou_…", "mail:neo@dpt.xyz"]
  ```
  An account key is `<kind>:<id>`, where the kind is `mail`, `telegram` or `lark`, as on Settings.
- **Membership.** Filing a workspace that names an account another active workspace already has is refused. Filing a project whose workspace does not exist is refused. Removing a workspace that active projects still name is refused.
- **Isolation (invariant 31).**
  - A draft may name a source thread. The thread's account must belong to the project's workspace, or the draft is refused.
  - The source travels in `Effect::Dispatch`, and the run checks it again before the first step.
  - A thread whose account is in no workspace is refused for every project.
- **Sign-ins (invariant 32).**
  - The GitHub token is under keychain service "Genatrix GitHub". The account is `token:<workspace>`, falling back to `token`.
  - An executor's token file is `~/.genatrix/<workspace>/<executor>.env` in the executor's user's home, falling back to `~/.genatrix/<executor>.env`.
  - Commands: `genatrix github login --workspace <name>` and `genatrix exec login <executor> --workspace <name>`.
- **High privilege (invariant 33).** `/api/work/define` already admits this machine only. Workspaces go through it.

## For the author, at the machine

1. Write `dpt.toml` with the name, the title and `lark:ou_1cef3807d41a4769f4af34db87dc865a` among its accounts, then run `genatrix --data-dir ~/.genatrix-dev workspace add dpt.toml`.
2. For each company repository, add `workspace = "dpt"` to its project file and run `genatrix project add <file>`.
3. If the company has its own GitHub account or Claude subscription, run `genatrix github login --workspace dpt` or `genatrix exec login claude-code --workspace dpt`.
4. Open More → Spaces → DPT. Its open Lark tasks are under "To do"; "Dispatch" on a task fills "New work" there, with only the company's projects.
