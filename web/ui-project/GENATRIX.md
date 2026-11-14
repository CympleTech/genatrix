# A Genatrix interface

This repository is one interface of Genatrix (design 15): the pages a person
sees on their devices. It is Svelte 5 and Vite. When your work is done,
Genatrix builds it itself and shows the person the result at once, as a
preview they can use or throw away. Nothing you change runs before that.

## Be quick

The person is waiting to see the change. Go straight to the page it is
about; do not survey the whole project. The task says which page they were
looking at. The pages:

| Address | File |
|---|---|
| `/` | `src/pages/Today.svelte` |
| `/approvals` | `src/pages/Approvals.svelte` |
| `/chats`, `/chats/<id>`, `/chats/g/<id>`, `/chats/t/<id>`, `/chats/a/<id>` | `src/pages/Chats.svelte`, with `src/components/Conversation.svelte` and `src/components/AgentChat.svelte` |
| `/ask` | `src/pages/Ask.svelte` |
| `/spaces`, `/spaces/<name>/...` | `src/pages/Spaces.svelte` |
| `/work/<id>` | `src/pages/Work.svelte` |
| `/timeline` | `src/pages/Timeline.svelte` |
| `/records` | `src/pages/Records.svelte` |
| `/review` | `src/pages/Review.svelte` |
| `/settings` | `src/pages/Settings.svelte`, with the panels in `src/components/` |
| `/welcome` | `src/pages/Welcome.svelte` |

The frame, the navigation and the routes are in `src/App.svelte`. Words on
the page are in `src/lib/i18n.ts`, in English and in Chinese: add both. Styles
are Tailwind classes and the rules in `src/app.css`.

## Checking and building

- Run `npm run check` once you are done. Fix what it reports.
- You do not need to build: Genatrix builds when you finish. You may run
  `npm run build` to see that it builds.
- `node_modules` is shared and read only, and there is no network. Do not add
  packages, and do not try to write into `node_modules`.
- If a check or the build fails for a reason that is not your code, say so in
  your summary and stop. Do not change `vite.config.ts`, `package.json`,
  `.npmrc`, `API.json` or this file to get around it.

## How it runs

- It runs inside Genatrix's shell, in a sandboxed frame. It has no origin of
  its own: no cookies, no browser storage, no network at all. Do not try to
  fetch anything, load fonts or images from elsewhere, open windows, or
  navigate away: the browser blocks all of it.
- Every request to Genatrix goes through the shell, as a message. The
  functions in `src/lib/api.ts` (`get`, `post`) and `src/lib/bridge.ts`
  already do this; use them and keep them as they are. `src/lib/router.ts`
  keeps the address bar in step; `src/lib/storage.ts` keeps small per-device
  settings.
- What Genatrix answers is in `API.json`: the stable, versioned data API
  (`/api/v1/...`) and the shell's messages. Prefer it to the older endpoints
  the pages use today when you add something; both work.
- Approving an action, typing the admin passphrase, pairing devices,
  exporting and erasing are the shell's, not yours: a card's "Review and
  decide" button asks the shell (`review` in `bridge.ts`). Lasting changes
  such as deleting a conversation are confirmed by the person in the shell
  when you ask for them.
- Links to elsewhere are handed to the shell, which shows the address first.

## Style

Change what the task asks for, in the style of the code around it. Phone
first; every state (loading, empty, error, long text) should look intended.
