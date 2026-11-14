// Design 15: the interface's side of the bridge to the shell.
//
// The interface runs in the shell's sandboxed frame. It holds no device
// credentials and can reach nothing but the shell: every request to the
// core is a message to the shell, which decides what to do with it, and
// every answer comes back the same way. Outside the frame (`npm run dev`,
// or the shell itself), nothing here is used.

export const framed: boolean = !import.meta.env.GENATRIX_SHELL && window.parent !== window;

/** What the shell answered: the core's status and body, as fetch gave them. */
export interface Answer { ok: boolean; status: number; body: any }

let next = 1;
const waiting = new Map<number, (a: Answer) => void>();
const listeners = new Set<(m: any) => void>();

if (framed) {
  window.addEventListener('message', (e) => {
    if (e.source !== window.parent) return;
    const m = e.data;
    if (!m || typeof m !== 'object') return;
    if (m.t === 'result' && typeof m.id === 'number') {
      const done = waiting.get(m.id);
      waiting.delete(m.id);
      done?.({ ok: !!m.ok, status: Number(m.status) || 0, body: m.body ?? null });
      return;
    }
    for (const l of listeners) l(m);
  });
}

/** Say something to the shell that wants no answer. */
export function tell(m: Record<string, unknown>) {
  window.parent.postMessage(m, '*');
}

/** Ask the shell something and wait for its answer. */
export function ask(kind: string, payload: Record<string, unknown>): Promise<Answer> {
  const id = next++;
  return new Promise((resolve) => {
    waiting.set(id, resolve);
    tell({ ...payload, t: kind, id });
  });
}

/** Hear what the shell says unasked: a new route, a change. */
export function hear(l: (m: any) => void): () => void {
  listeners.add(l);
  return () => listeners.delete(l);
}

/** A request to the core, through the shell. */
export function request(method: string, path: string, body?: unknown): Promise<Answer> {
  return ask('call', { method, path, body: body === undefined ? null : body });
}

/** Ask the shell to open an action for your decision; the action as it stands after. */
export async function review(id: string): Promise<any | null> {
  const a = await ask('review', { action: id });
  return a.ok ? a.body : null;
}

/** Ask the shell to open one of its own panels. */
export function openShell(panel: 'security' | 'interface' | 'pair') {
  tell({ t: 'panel', panel });
}

// Links: the frame may not navigate away or open windows. A link to
// anywhere else, or a download from the core, is handed to the shell, which
// shows where it goes before it opens it.
if (framed) {
  document.addEventListener('click', (e) => {
    const a = (e.target as Element | null)?.closest?.('a[href]') as HTMLAnchorElement | null;
    if (!a) return;
    const href = a.getAttribute('href') ?? '';
    if (href.startsWith('#')) return;
    e.preventDefault();
    const url = new URL(href, document.baseURI);
    if (url.pathname.startsWith('/api/')) tell({ t: 'open', url: url.pathname + url.search, download: a.hasAttribute('download') });
    else if (url.protocol === 'http:' || url.protocol === 'https:') tell({ t: 'open', url: url.href });
  }, true);
}
