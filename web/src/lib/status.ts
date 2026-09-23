// The numbers at the top, and how many actions wait: asked every five
// seconds, shared by every screen.
import { writable } from 'svelte/store';
import { get, type Status, type AccountState } from './api';

export const status = writable<Status | null>(null);
export const accounts = writable<AccountState[]>([]);
export const pending = writable<number>(0);
export const unauthorized = writable<boolean>(false);
export const statusError = writable<string>('');

// Two cheap calls, in parallel. The count of waiting approvals rides along
// on the status, because polling Today for it meant reading every open
// promise in the store every five seconds, behind the one lock everything
// else queues on.
async function tick() {
  try {
    const [s, a] = await Promise.all([
      get<Status>('/api/status'),
      get<{ accounts: AccountState[] }>('/api/accounts'),
    ]);
    status.set(s);
    accounts.set(a.accounts);
    pending.set(s.pending_actions);
    unauthorized.set(false);
    statusError.set('');
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
