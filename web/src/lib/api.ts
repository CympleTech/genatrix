// The core's JSON interface. Every call goes to the same origin; a paired
// device carries its cookie, loopback needs nothing.

export class ApiError extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}

async function read(response: Response): Promise<any> {
  const text = await response.text();
  let body: any = null;
  try { body = text ? JSON.parse(text) : null; } catch { body = null; }
  if (!response.ok) {
    const message = (body && body.error) || response.statusText || `HTTP ${response.status}`;
    throw new ApiError(response.status, message);
  }
  return body;
}

export async function get<T = any>(path: string): Promise<T> {
  return read(await fetch(path, { credentials: 'same-origin' }));
}

export async function post<T = any>(path: string, body: unknown = {}): Promise<T> {
  return read(await fetch(path, {
    method: 'POST',
    credentials: 'same-origin',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  }));
}

// --- shapes the core sends (crates/daemon/src/web/api.rs) ---

export interface SourceRef { id: string; at: string; connector: string; who: string }
export interface Row {
  id: string; at: string; connector: string; direction: string; author: string;
  thread: string; level: string; level_reason: string; preview: string; has_more: boolean;
}
export interface Judgement { by: string; detail: string; level: string; at: string }
export interface Detail {
  row: Row; text: string; subject: string | null; recipients: string[];
  judgements: Judgement[]; summary: string | null; tombstoned: boolean;
}
export interface Point { text: string; sources: SourceRef[] }
export interface Group { group: 'needs_reply' | 'promised' | 'worth_knowing'; points: Point[] }
export interface Digest { day: string; generated_at: string; considered: number; groups: Group[] }
export interface Commitment {
  id: string; what: string; due: string | null; from: string; to: string | null; mine: boolean;
  status: 'open' | 'done' | 'cancelled' | 'overdue'; standing: 'inferred' | 'confirmed' | 'rejected';
  evidence: SourceRef[];
}
export interface Action {
  id: string; kind: string; target: string; account: string; rationale: string;
  evidence: SourceRef[]; draft: string; version: number; payload_hash: string;
  status: string; status_detail: string; created_at: string; expires_in_secs: number;
  nonce: string | null; drafted: string; versions: number; can_withdraw: boolean;
  result: SourceRef | null;
}
export interface Today { digest: Digest | null; commitments: Commitment[]; commitments_total: number; actions: Action[]; pending_actions: number }
export interface Handle { kind: string; value: string; inferred: boolean }
export interface PersonCard {
  id: string; name: string; handles: Handle[]; from_them: number; to_them: number;
  last_at: string | null; connectors: string[]; roles: string[]; last_text: string | null;
}
export interface Stats {
  from_them: number; to_them: number; first_at: string | null; last_at: string | null;
  months: number[]; reply_hours: number | null; language: string; connectors: [string, number][];
}
export interface PersonDetail { card: PersonCard; stats: Stats; notes: string; recent: Row[]; commitments: Commitment[] }
export interface ReviewRow { row: Row; subject: string | null; text: string; model_level: string }
export interface Review { items: ReviewRow[]; tally: { judged: number; reviewed: number; agreed: number } }
export interface AccountState { address: string; sync: { state: string; [k: string]: unknown }; text: string }
export interface Status {
  items: number; ledger_entries: number; cloud_enabled: boolean; rules_version: string;
  bytes_left_device: number; bytes_on_disk: number; data_dir: string;
  model: { state: string; [k: string]: unknown }; model_text: string;
  embedded: number; summarized: number; judged: number; pending_actions: number;
}
export interface Call {
  at: string; purpose: string; level: string; target: string; location: string;
  decision: string; items: number; payload: string | null;
}
export interface RunLine { id: string; at: string; task: string; max_steps: number; end: string; steps: number; reason: string }
export interface ActionEvent { at: string; action: string; event: string; by: string; kind: string; version: number; detail: string }
export interface Ledger { headline: string; entries: number; verified: boolean; calls: Call[]; runs: RunLine[]; actions: ActionEvent[] }
export interface RunStep { at: string; kind: string; body: any }
export interface AskReply {
  run_id: string; answer: string; cited: SourceRef[]; steps: string[];
  actions: Action[]; stopped: boolean; answered: string;
}
export interface Device { id: string; name: string; created_at: string; last_seen: string | null; revoked_at: string | null; this: boolean }

export interface ChatMessage {
  id: string; day: string; time: string; ms: number; mine: boolean; connector: string;
  place: 'mail' | 'direct' | 'group' | 'channel'; place_name: string | null;
  subject: string | null; text: string; folded: boolean; level: string;
}
export interface ChatPage { messages: ChatMessage[]; earlier: boolean }
