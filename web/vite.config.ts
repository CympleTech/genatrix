import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import tailwindcss from '@tailwindcss/vite';

// The build lands beside the Rust code that compiles it into the binary
// (crates/daemon/src/web/mod.rs), under fixed names, so the include_str! calls
// there never chase a hash. One script, one stylesheet, no chunks.
export default defineConfig({
  plugins: [tailwindcss(), svelte()],
  build: {
    outDir: '../crates/daemon/src/web/dist',
    emptyOutDir: true,
    assetsDir: '.',
    rollupOptions: {
      output: {
        entryFileNames: 'app.js',
        assetFileNames: 'app.[ext]',
        inlineDynamicImports: true,
      },
    },
  },
  server: {
    // `npm run dev` talks to a running core on the usual port.
    proxy: { '/api': 'http://127.0.0.1:7717' },
  },
});
