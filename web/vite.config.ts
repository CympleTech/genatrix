import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import tailwindcss from '@tailwindcss/vite';

// Design 15: two builds from one source. The default interface (`app`)
// runs inside the shell's isolated frame and is served by hash under
// /ui/<hash>/, so its paths are relative. The shell (`--mode shell`) is the
// top-level page: approvals, the passphrase, pairing, devices, and the frame.
// Both land beside the Rust code that compiles them into the binary
// (crates/daemon/src/web/mod.rs and ui.rs), under fixed names, so the
// include_str! calls there never chase a hash. One script, one stylesheet,
// no chunks, each.
export default defineConfig(({ mode }) => {
  const shell = mode === 'shell';
  return {
    plugins: [tailwindcss(), svelte()],
    base: shell ? '/' : './',
    publicDir: shell ? 'public' : false,
    define: { 'import.meta.env.GENATRIX_SHELL': JSON.stringify(shell) },
    build: {
      outDir: shell ? '../crates/daemon/src/web/dist/shell' : '../crates/daemon/src/web/dist/ui',
      emptyOutDir: true,
      assetsDir: '.',
      rollupOptions: {
        input: shell ? 'shell.html' : 'index.html',
        output: {
          entryFileNames: shell ? 'shell.js' : 'app.js',
          assetFileNames: shell ? 'shell.[ext]' : 'app.[ext]',
          inlineDynamicImports: true,
        },
      },
    },
    server: {
      // `npm run dev` talks to a running core on the usual port.
      proxy: { '/api': 'http://127.0.0.1:7717' },
    },
  };
});
