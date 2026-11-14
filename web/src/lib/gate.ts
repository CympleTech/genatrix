// Design 15, "数据出不去": what the shell does with each request the
// interface sends. The list is the boundary: anything not on it is asked
// about, never passed.

export type Way =
  /** Sent as it is: reading, drafting, asking. */
  | 'pass'
  /** Shown to you in the shell first; sent once you say so. */
  | 'confirm'
  /** An approval: the shell opens the action itself. */
  | 'review'
  /** Done only in the shell's own panels. */
  | 'shell';

export interface Verdict { way: Way; what: string; target?: string }

type Rule = [method: string, pattern: RegExp, way: Way, what: string];

const RULES: Rule[] = [
  // Approvals are decided in the shell, on the card it fetches itself.
  ['POST', /^\/api\/action\/([^/]+)\/(approve|decline|edit|level)$/, 'review', 'review'],
  // The shell's own: devices, pairing, the passphrase, the whole of the data,
  // and which interface runs.
  ['POST', /^\/api\/admin\//, 'shell', 'admin'],
  ['POST', /^\/api\/device\//, 'shell', 'devices'],
  ['POST', /^\/api\/pair(\/start)?$/, 'shell', 'pair'],
  ['POST', /^\/api\/local\//, 'shell', 'local'],
  ['POST', /^\/api\/(erase|export)$/, 'shell', 'erase'],
  ['POST', /^\/api\/ui(\/|$)/, 'shell', 'interface'],
  ['GET', /^\/api\/ui(\/|$)/, 'shell', 'interface'],
  // High-privilege, or not to be undone: asked first.
  ['POST', /^\/api\/setup\/mail$/, 'confirm', 'add_mailbox'],
  ['POST', /^\/api\/setup\/telegram\/start$/, 'confirm', 'start_telegram'],
  ['POST', /^\/api\/setup\/lark\/start$/, 'confirm', 'start_lark'],
  ['POST', /^\/api\/setup\/remove$/, 'confirm', 'remove_account'],
  ['POST', /^\/api\/agents\/install$/, 'confirm', 'install_agent'],
  ['POST', /^\/api\/agent\/([^/]+)\/uninstall$/, 'confirm', 'uninstall_agent'],
  ['POST', /^\/api\/work\/define$/, 'confirm', 'file_definition'],
  ['POST', /^\/api\/github\/token$/, 'confirm', 'github_sign_in'],
  ['POST', /^\/api\/github\/forget$/, 'confirm', 'github_sign_out'],
  ['POST', /^\/api\/model\/download$/, 'confirm', 'download_model'],
  ['POST', /^\/api\/person\/([^/]+)\/delete$/, 'confirm', 'delete_person'],
  ['POST', /^\/api\/group\/([^/]+)\/delete$/, 'confirm', 'delete_group'],
  ['POST', /^\/api\/person\/([^/]+)\/junk$/, 'confirm', 'junk_person'],
  ['POST', /^\/api\/group\/([^/]+)\/junk$/, 'confirm', 'junk_group'],
  ['POST', /^\/api\/junk\/([^/]+)\/([^/]+)\/forget$/, 'confirm', 'junk_forget'],
  ['POST', /^\/api\/work\/([^/]+)\/stop$/, 'confirm', 'stop_work'],
  ['POST', /^\/api\/item\/([^/]+)\/level$/, 'confirm', 'item_level'],
  // Ordinary: what comes of them waits for your approval, or is only read.
  ['POST', /^\/api\/item\/[^/]+\/draft$/, 'pass', ''],
  ['POST', /^\/api\/commitment\/[^/]+$/, 'pass', ''],
  ['POST', /^\/api\/person\/[^/]+\/(category|relationship)$/, 'pass', ''],
  ['POST', /^\/api\/ask$/, 'pass', ''],
  ['POST', /^\/api\/agent\/[^/]+\/(message|pause|resume)$/, 'pass', ''],
  ['POST', /^\/api\/work\/[^/]+\/(answer|push-again)$/, 'pass', ''],
  ['POST', /^\/api\/work\/(branches|draft|suggest)$/, 'pass', ''],
  ['POST', /^\/api\/junk\/[^/]+\/[^/]+\/restore$/, 'pass', ''],
  ['POST', /^\/api\/setup\/telegram\/(code|password)$/, 'pass', ''],
  ['POST', /^\/api\/setup\/lark\/poll$/, 'pass', ''],
];

/** What to do with one request from the interface. */
export function judge(method: string, path: string): Verdict {
  const m = method.toUpperCase();
  const bare = path.split('?')[0];
  if (!bare.startsWith('/api/') || bare.includes('..')) return { way: 'shell', what: 'nowhere' };
  for (const [rm, re, way, what] of RULES) {
    if (rm !== m) continue;
    const hit = re.exec(bare);
    if (hit) return { way, what, target: hit[1] };
  }
  // Reading is passed; anything else not on the list is asked about.
  if (m === 'GET') return { way: 'pass', what: '' };
  return { way: 'confirm', what: 'unknown' };
}

/** Field names whose values are not shown back, only that they were given. */
const SECRET = /pass|token|secret|code|key|password/i;

/** The fields of a request, as the shell shows them before you confirm. */
export function shown(body: unknown): [string, string][] {
  if (!body || typeof body !== 'object') return [];
  const out: [string, string][] = [];
  const walk = (prefix: string, v: unknown) => {
    if (v === null || v === undefined || v === '') return;
    if (typeof v === 'object' && !Array.isArray(v)) {
      for (const [k, x] of Object.entries(v as Record<string, unknown>)) walk(prefix ? `${prefix}.${k}` : k, x);
      return;
    }
    const last = prefix.split('.').pop() ?? prefix;
    const text = Array.isArray(v) ? v.map((x) => (typeof x === 'object' ? JSON.stringify(x) : String(x))).join(', ') : String(v);
    out.push([prefix, SECRET.test(last) ? '••••' : text.length > 400 ? text.slice(0, 400) + '…' : text]);
  };
  walk('', body);
  return out;
}
