// The numbers at the top, and how many actions wait: asked every five
// seconds, shared by every screen.
import { writable } from 'svelte/store';
import { get, type Status, type AccountState } from './api';

export const status = writable<Status | null>(null);
export const accounts = writable<AccountState[]>([]);
export const pending = writable<number>(0);
export const unauthorized = writable<boolean>(false);
export const statusError = writable<string>('');

async function tick() {
  try {
    const s = await get<Status>('/api/status');
    status.set(s);
    unauthorized.set(false);
    statusError.set('');
    const a = await get<{ accounts: AccountState[] }>('/api/accounts');
    accounts.set(a.accounts);
    const t = await get<{ pending_actions: number }>('/api/today');
    pending.set(t.pending_actions);
  } catch (e: any) {
    if (e && e.status === 401) unauthorized.set(true);
    else statusError.set(e?.message || String(e));
  }
}

export function startPolling() {
  tick();
  setInterval(tick, 5000);
}

export function refreshPending() { tick(); }
