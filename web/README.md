# The interface

Design: `docs/design/06-interface.md` (the built-in interface) and
`docs/design/15-custom-interface.md` (the shell, and interfaces you change).
Svelte and Vite. One source, two builds, both under fixed names in
`crates/daemon/src/web/dist/` and compiled into the core binary, so a Rust
toolchain alone builds Genatrix. The built files are committed with the
source: after changing anything here, run the build and commit `dist/` in the
same change.

- `dist/shell/` is the **shell** (`shell.html`, `src/shell.ts`,
  `src/Shell.svelte`): the trusted top-level page. Approvals, the passphrase,
  pairing, devices, export and erase, and which interface runs are drawn here.
  It loads the interface in a sandboxed frame and passes on its requests
  according to `src/lib/gate.ts`.
- `dist/ui/` is the **built-in interface** (`index.html`, `src/main.ts`,
  `src/App.svelte`): everything else. Inside the frame it has no origin, no
  credentials and no network; `src/lib/bridge.ts` carries its requests to the
  shell. Its paths are relative, because it is served under `/ui/<hash>/`.

A changed interface is filed with `genatrix ui add <name> <dir>` (the `dist/ui`
of a build) and approved in the shell's Interface panel. The build also packs
the built-in interface's source (`scripts/pack-source.mjs`, with the files in
`ui-project/`) into `dist/ui-source.json`: copying the built-in interface
starts a repository from it, which an executor changes when you ask in the
Interface panel. The data it reads is described in
`crates/daemon/src/web/v1.json`.

```sh
npm ci            # once
npm run build     # -> ../crates/daemon/src/web/dist/{ui,shell}
npm run check     # types
npm test          # the shell's gate (Node's own test runner)
npm run dev       # the interface alone, unframed, proxying /api to a core on 127.0.0.1:7717
```

Phone first: the layout is designed at phone width and widens for a desktop.
The page is what a paired phone opens and what the menu bar shell shows in
its window.
