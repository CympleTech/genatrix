// Design 15: the built-in interface's source, packed for the core, which
// starts each interface's own repository from it. The shell's files stay
// out: an interface never carries the shell. Run by `npm run build`.
import { readFileSync, readdirSync, statSync, writeFileSync } from 'node:fs';
import { join, relative } from 'node:path';

const here = new URL('..', import.meta.url).pathname;
const SHELL_ONLY = new Set(['src/shell.ts', 'src/Shell.svelte', 'src/lib/gate.ts', 'src/components/InterfacePanel.svelte', 'src/components/ChatBubble.svelte']);
const files = {};

function walk(dir) {
  for (const name of readdirSync(dir)) {
    const full = join(dir, name);
    if (statSync(full).isDirectory()) walk(full);
    else {
      const rel = relative(here, full);
      if (!SHELL_ONLY.has(rel)) files[rel] = readFileSync(full, 'utf8');
    }
  }
}
walk(join(here, 'src'));
for (const f of ['index.html', 'tsconfig.json', 'svelte.config.js', 'package-lock.json']) {
  try { files[f] = readFileSync(join(here, f), 'utf8'); } catch { /* not every project has each */ }
}
const pkg = JSON.parse(readFileSync(join(here, 'package.json'), 'utf8'));
pkg.name = 'genatrix-interface';
pkg.description = 'A Genatrix interface (design 15).';
// The packages are shared and read only: Vite must not write its config
// beside them (design 15 v0.6), so it loads the config without a temp file.
pkg.scripts = { build: 'vite build --configLoader runner', check: 'svelte-check --tsconfig ./tsconfig.json' };
files['package.json'] = JSON.stringify(pkg, null, 2) + '\n';
files['vite.config.ts'] = readFileSync(join(here, 'ui-project/vite.config.ts'), 'utf8');
files['GENATRIX.md'] = readFileSync(join(here, 'ui-project/GENATRIX.md'), 'utf8');
files['.gitignore'] = readFileSync(join(here, 'ui-project/gitignore'), 'utf8');
files['.npmrc'] = readFileSync(join(here, 'ui-project/npmrc'), 'utf8');
const out = join(here, '../crates/daemon/src/web/dist/ui-source.json');
writeFileSync(out, JSON.stringify(files));
console.log(`packed ${Object.keys(files).length} source files`);
