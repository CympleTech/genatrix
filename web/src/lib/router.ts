// Paths, not hashes, so a link to /approvals or /pair?code=… opens on the
// right screen; the core serves the page for every non-API path.
import { writable } from 'svelte/store';

export interface Route { path: string; parts: string[]; query: URLSearchParams }

function current(): Route {
  const path = location.pathname.replace(/\/+$/, '') || '/';
  return { path, parts: path.split('/').filter(Boolean), query: new URLSearchParams(location.search) };
}

export const route = writable<Route>(current());

export function go(path: string, replace = false) {
  if (replace) history.replaceState(null, '', path); else history.pushState(null, '', path);
  route.set(current());
}

window.addEventListener('popstate', () => route.set(current()));
