import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import tailwindcss from '@tailwindcss/vite';

// A Genatrix interface (design 15). `npm run build` leaves it in `dist/`,
// which Genatrix files as a new version of this interface once the work is
// done; you approve it in the shell's Interface panel. Paths are relative:
// it is served under /ui/<hash>/. One script, one stylesheet, no chunks.
export default defineConfig({
  plugins: [tailwindcss(), svelte()],
  base: './',
  publicDir: false,
  cacheDir: '.vite-cache',
  define: { 'import.meta.env.GENATRIX_SHELL': 'false' },
  build: {
    outDir: 'dist',
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
});
