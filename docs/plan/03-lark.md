# Lark tasks

Design 05 v0.3, "飞书（Lark）：你负责的任务", awaiting review. The aim is for the tasks assigned to the author in their company's Lark to reach the executors of design 13. There are three steps, and each one is useful without the next.

## Steps

| Step | What | State |
|---|---|---|
| L1 | Read-only connector: the tasks assigned to you and their comments reach the timeline, the digest and search | Built and tested with a stand-in for Lark. Not yet run against a real tenant |
| L2 | A project names the Lark task lists it takes work from. A new task in one of them becomes a dispatch draft, never a dispatch. Design 13's "新 Item" trigger, and the ruling on company content (a per-project switch: this project's own tasks and code may go to its executors) | Not started; design 13 first |
| L3 | Write-back: comment the pull request link on the task, mark it done. Each is an approved action; the app gains write permissions only then | Not started |

## L1 as built

```
crates/connectors/lark     genatrix-lark: one process for every Lark account, sandboxed like Telegram
  auth.rs                  device flow (RFC 8628), refresh, who am I; used by the core to sign in
  api.rs                   the read calls, behind a `Remote` trait
  normalize.rs             Lark JSON into `TaskRecord` and `TaskComment`
  sync.rs                  one round: what changed since the last; the position is the account's cursor
crates/daemon              accounts (`[lark]` in accounts.toml), ingest (task, comment), the page's sign-in
```

- **Model.** `Kind::Task` with `Payload::Task`, `ThreadKind::Task`, `HandleKind::LarkId` and `Connector::Lark`. These are stored as text columns without CHECK constraints, so no migration was needed. A task is its own thread. Each change to it is a new version that supersedes the last. Comments are message items in the task's thread.
- **Protocol.** `Store` gains `tasks` and `comments`, fields 4 and 5. Older connectors leave them empty.
- **Secrets.**
  - The App Secret is in the keychain under "Genatrix Lark", keyed by App ID.
  - The sign-in, which rotates on every refresh, is in the encrypted store as the account's `oauth` cursor, beside the `tasks` position. This is the same arrangement as the Telegram session.
  - The connector saves a renewed sign-in before it uses it. If that save fails, the account asks for a new sign-in.
- **Sign-in.**
  - From the terminal: `genatrix account --add-lark <app id> [--feishu]`.
  - From Settings on this machine: a link and a code, confirmed in Lark on any device.
  - Invariant 12 covers both page routes.
- **Reading.** Open tasks every two minutes. A task's comments are read when it changes, and every ten minutes for all open tasks. Done tasks are read once, as history. Names come from the contacts API when the app has the permission; a name is asked for once.
- **Sandbox.** Port 443, as for Telegram. The connector checks that the host is its own open platform.

## For the author, at the machine

1. In the Lark developer console (open.larksuite.com/app), create a custom app.
2. Under Permissions, add `task:task:read`, `task:comment:read`, `task:tasklist:read`, `contact:contact.base:readonly`, `contact:user.base:readonly` and `offline_access`. Names need both contact permissions: the first lets the user lookup be called as the user, the second puts the name in its answer. Publish a version and have the admin approve it.
3. `genatrix --data-dir ~/.genatrix-dev account --add-lark cli_...`, then paste the App Secret, open the link and confirm. Or use Settings → Sign in to Lark on the Mac.
4. Restart `serve`. On Settings the account should go from connecting to live. The tasks should then show up in the conversation with whoever assigned them, marked "Lark task".
5. If it fails, read the account's line on Settings first. Lark's error code names the missing permission or the step that failed.
