import { t } from './i18n';
import type { AdminLine } from './api';

// One high-privilege request in words (design 06 v0.7). The core records a
// code and what it was about; the page says it in its own language.
export function actWords(l: AdminLine): string {
  return t(`admin.act.${l.act}`, { s: l.subject ?? '' }).trim();
}
export function byWords(l: AdminLine): string {
  return l.device ? l.by : t('admin.here');
}
export function adminLine(l: AdminLine): string {
  return t(`admin.news.${l.outcome}`, { what: actWords(l), by: byWords(l) });
}
