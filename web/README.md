# The interface

Design: `docs/design/06-interface.md`. Svelte and Vite; the build lands in
`crates/daemon/src/web/dist/` under fixed names and is compiled into the core
binary, so a Rust toolchain alone builds Genatrix. The built files are
committed with the source: after changing anything here, run the build and
commit `dist/` in the same change.

```sh
npm ci            # once
npm run build     # -> ../crates/daemon/src/web/dist
npm run dev       # a dev server proxying /api to a core on 127.0.0.1:7717
```

Phone first: the layout is designed at phone width and widens for a desktop.
The page is what a paired phone opens and what the menu bar shell shows in
its window; it is the only interface.
