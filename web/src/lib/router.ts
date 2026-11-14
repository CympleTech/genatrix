// Paths, not hashes, so a link to /approvals or /pair?code=… opens on the
// right screen; the core serves the page for every non-API path.
//
// Design 15: inside the shell's frame the address bar is the shell's. The
// frame starts on the path the shell put after `#` in its address, tells
// the shell when it moves, and hears from the shell when you go back.
import { writable } from 'svelte/store';
import { framed, hear, tell } from './bridge';

export interface Route { path: string; parts: string[]; query: URLSearchParams }

function parse(full: string): Route {
  const at = full.indexOf('?');
  const raw = at < 0 ? full : full.slice(0, at);
  const path = raw.replace(/\/+$/, '') || '/';
  return { path, parts: path.split('/').filter(Boolean), query: new URLSearchParams(at < 0 ? '' : full.slice(at + 1)) };
}

function current(): Route {
  if (framed) return parse(decodeURIComponent(location.hash.slice(1)) || '/');
  return parse(location.pathname + location.search);
}

export const route = writable<Route>(current());

export function go(path: string, replace = false) {
  if (framed) {
    route.set(parse(path));
    tell({ t: 'nav', path, replace });
    return;
  }
  if (replace) history.replaceState(null, '', path); else history.pushState(null, '', path);
  route.set(current());
}

if (framed) {
  hear((m) => { if (m.t === 'route' && typeof m.path === 'string') route.set(parse(m.path)); });
} else {
  window.addEventListener('popstate', () => route.set(current()));
}
