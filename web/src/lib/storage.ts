// Small things a device remembers: the language, a banner seen, a step
// skipped. Design 15: inside the shell's frame there is no browser storage
// (the frame has no origin of its own), so the shell keeps them for it: it
// hands over what it holds in the frame's address, and is told each change.
import { framed, tell } from './bridge';

const PREFIX = 'genatrix.frame.';

function snapshot(): Record<string, string> {
  try {
    const s = new URLSearchParams(location.search).get('s');
    const parsed = s ? JSON.parse(decodeURIComponent(escape(atob(s)))) : {};
    return parsed && typeof parsed === 'object' ? parsed : {};
  } catch { return {}; }
}
const held: Record<string, string> = framed ? snapshot() : {};

export function getItem(key: string): string | null {
  if (framed) return held[key] ?? null;
  try { return localStorage.getItem(key); } catch { return null; }
}

export function setItem(key: string, value: string) {
  if (framed) { held[key] = value; tell({ t: 'store', key, value }); return; }
  try { localStorage.setItem(key, value); } catch { /* nothing to keep it in */ }
}

export function removeItem(key: string) {
  if (framed) { delete held[key]; tell({ t: 'store', key, value: null }); return; }
  try { localStorage.removeItem(key); } catch { /* nothing to keep it in */ }
}

/** Show the page again, as it would be opened now. */
export function reload() {
  if (framed) tell({ t: 'reload' }); else location.reload();
}

// --- the shell's side ---

/** What the shell keeps for the frame, packed for its address. */
export function packForFrame(): string {
  const out: Record<string, string> = {};
  try {
    for (let i = 0; i < localStorage.length; i++) {
      const k = localStorage.key(i);
      if (k && k.startsWith(PREFIX)) out[k.slice(PREFIX.length)] = localStorage.getItem(k) ?? '';
    }
  } catch { /* nothing kept */ }
  return btoa(unescape(encodeURIComponent(JSON.stringify(out))));
}

/** Keep one of the frame's things: small values, plain keys, at most fifty. */
export function keepForFrame(key: unknown, value: unknown) {
  if (typeof key !== 'string' || !/^[\w.-]{1,64}$/.test(key)) return;
  try {
    if (value === null) { localStorage.removeItem(PREFIX + key); return; }
    if (typeof value !== 'string' || value.length > 4096) return;
    let n = 0;
    for (let i = 0; i < localStorage.length; i++) if (localStorage.key(i)?.startsWith(PREFIX)) n++;
    if (n >= 50 && localStorage.getItem(PREFIX + key) === null) return;
    localStorage.setItem(PREFIX + key, value);
  } catch { /* nothing to keep it in */ }
}
