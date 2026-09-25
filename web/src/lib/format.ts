import { t } from './i18n';
import type { Action, CardValue } from './api';

export const LEVELS = ['public', 'personal', 'secret'] as const;

export function connectorName(c: string): string {
  return t(`connector.${c}`) === `connector.${c}` ? c : t(`connector.${c}`);
}

export function levelWord(level: string): string {
  return t(`level.${level}`);
}

export function kindWord(kind: string): string {
  const key = `action.kind.${kind}`;
  return t(key) === key ? kind : t(key);
}

export function expiresIn(secs: number): string {
  if (secs <= 0) return t('expires.expired');
  const d = Math.floor(secs / 86400), h = Math.floor((secs % 86400) / 3600);
  if (d >= 1) return d === 1 ? t('expires.day') : t('expires.days', { n: d });
  if (h >= 1) return h === 1 ? t('expires.hour') : t('expires.hours', { n: h });
  return t('expires.soon');
}

// What each state means to the person reading it. "unknown" is the one that
// matters most: it must never read like a failure to be retried (design 05).
export function statusLine(a: Action): string {
  if (a.status === 'pending') return expiresIn(a.expires_in_secs);
  // An agent's own effect is done or not done, never sent.
  const own = `action.status.agent.${a.status}`;
  const key = a.kind === 'agent' && t(own) !== own ? own : `action.status.${a.status}`;
  const word = t(key) === key ? a.status : t(key);
  return a.status_detail ? `${word} · ${a.status_detail}` : word;
}

/** A card value as text. Money is minor units; shown with two decimals. */
export function cardValue(v: CardValue): string {
  switch (v.type) {
    case 'text': return v.text;
    case 'date': return v.date;
    case 'money': {
      const sign = v.cents < 0 ? '−' : '';
      const abs = Math.abs(v.cents);
      return `${sign}${v.currency} ${Math.floor(abs / 100).toLocaleString()}.${String(abs % 100).padStart(2, '0')}`;
    }
  }
}

export function hours(h: number | null): string {
  if (h == null) return '—';
  if (h < 1) return `${Math.round(h * 60)} min`;
  if (h < 48) return `${h.toFixed(1)} h`;
  return `${(h / 24).toFixed(1)} d`;
}

export function initials(name: string): string {
  const clean = (name || '').trim();
  if (!clean) return '?';
  const parts = clean.split(/\s+/);
  const first = [...parts[0]][0] || '?';
  const second = parts.length > 1 ? [...parts[parts.length - 1]][0] : '';
  return (first + second).toUpperCase();
}

export function bytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(0)} MB`;
  return `${(n / 1024 / 1024 / 1024).toFixed(1)} GB`;
}
