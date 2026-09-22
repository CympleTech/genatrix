// Genatrix, local interface.
//
// Plain JavaScript on purpose: there is nothing here that needs a framework,
// and nothing that needs a build step. Fetch some JSON, put it on the page.

const $ = (sel) => document.querySelector(sel);

async function get(path) {
  const response = await fetch(path);
  const body = await response.json();
  if (!response.ok) throw new Error(body.error || response.statusText);
  return body;
}

function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

// --- tabs -------------------------------------------------------------

for (const tab of document.querySelectorAll('.tab')) {
  tab.addEventListener('click', () => {
    for (const other of document.querySelectorAll('.tab')) other.classList.remove('is-on');
    for (const pane of document.querySelectorAll('.pane')) pane.classList.remove('is-on');
    tab.classList.add('is-on');
    $('#' + tab.dataset.pane).classList.add('is-on');
    if (tab.dataset.pane === 'records') loadRecords();
    if (tab.dataset.pane === 'review') loadReview();
    if (tab.dataset.pane === 'today') loadToday();
    if (tab.dataset.pane === 'people') loadPeople();
    if (tab.dataset.pane === 'approvals') loadApprovals();
    if (tab.dataset.pane === 'timeline') loadTimeline();
  });
}

async function post(path, body) {
  const response = await fetch(path, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  });
  const data = await response.json();
  if (!response.ok) throw new Error(data.error || response.statusText);
  return data;
}

// --- levels -----------------------------------------------------------

const LEVELS = ['public', 'personal', 'secret'];

// Design 06: three options. Raising takes effect on the click. Lowering
// says what it means in the option itself, and the click on that line is
// the confirmation; no modal, no second "OK".
function levelChooser(itemId, current, agreeWith, onDone) {
  const box = el('div', 'chooser');
  for (const level of LEVELS) {
    const rank = LEVELS.indexOf(level) - LEVELS.indexOf(current);
    let label;
    if (level === agreeWith) label = `Agree: ${level}`;
    else if (rank > 0) label = `Raise to ${level}`;
    else if (rank < 0) label = `Lower to ${level}: could later go to the cloud, redacted`;
    else label = `Keep ${level}`;
    const button = el('button', `choose level-${level}` + (rank < 0 ? ' lowers' : ''), label);
    button.type = 'button';
    button.addEventListener('click', async () => {
      for (const b of box.querySelectorAll('button')) b.disabled = true;
      try {
        const d = await post(`/api/item/${itemId}/level`, { level });
        onDone(d);
      } catch (e) {
        box.append(el('span', 'error', e.message));
      }
    });
    box.append(button);
  }
  return box;
}

// --- status -----------------------------------------------------------

let knownItems = null;

async function loadStatus() {
  try {
    const s = await get('/api/status');
    const bytes = s.bytes_left_device === 0
      ? 'nothing has left this device'
      : `${s.bytes_left_device} bytes have left this device`;
    $('#status').textContent =
      `${s.items} items · ${s.judged} judged · ${s.embedded} embedded · ${s.summarized} summarized · model ${s.model_text} · cloud ${s.cloud_enabled ? 'on' : 'off'} · ${bytes}`;
    // New mail shows up on the timeline without a reload.
    if (knownItems !== null && s.items !== knownItems && $('#timeline').classList.contains('is-on')) {
      loadTimeline();
    }
    knownItems = s.items;
  } catch (e) {
    $('#status').textContent = e.message;
    $('#status').classList.add('error');
  }
  await loadAccounts();
}

// One line per account: what it is doing, in the connector's own words.
async function loadAccounts() {
  const list = $('#accounts');
  try {
    const { accounts } = await get('/api/accounts');
    list.replaceChildren();
    for (const a of accounts) {
      const row = el('li', 'account ' + a.sync.state);
      row.append(el('span', 'address', a.address), el('span', 'state', a.text));
      list.append(row);
    }
  } catch (e) {
    list.replaceChildren(el('li', 'account error', e.message));
  }
}

// --- timeline ---------------------------------------------------------

function levelMarker(level, reason) {
  const node = el('span', `level level-${level}`, level);
  if (reason) node.title = reason;
  return node;
}

function rowNode(row) {
  const li = el('li', 'row');

  const meta = el('div', 'meta');
  meta.append(
    el('time', null, row.at),
    levelMarker(row.level, row.level_reason),
    el('span', 'who', row.author),
    el('span', null, row.connector === 'imap' ? 'mail' : row.connector),
  );
  // A direct chat is named after the person in it, so showing both is noise.
  if (row.thread && row.thread !== row.author) {
    meta.append(el('span', 'thread', row.thread));
  }
  li.append(meta);

  const preview = el('p', 'preview', row.preview + (row.has_more ? '…' : ''));
  li.append(preview);

  let detail = null;
  preview.addEventListener('click', async () => {
    if (detail) {
      detail.remove();
      detail = null;
      li.classList.remove('is-open');
      return;
    }
    li.classList.add('is-open');
    detail = el('div', 'detail');
    detail.append(el('p', null, 'loading…'));
    li.append(detail);
    try {
      const d = await get(`/api/item/${row.id}`);
      detail.replaceChildren();
      if (d.subject) detail.append(el('p', null, d.subject));
      // The model's summary, marked as its own and kept apart from the text
      // (design 06). Never mixed into the content.
      if (d.summary) {
        const summary = el('div', 'summary');
        summary.append(el('span', 'ai', 'AI summary'), el('p', null, d.summary));
        detail.append(summary);
      }
      detail.append(el('p', null, d.text));
      if (d.tombstoned) {
        detail.append(el('p', 'note', 'Deleted by the sender; kept here.'));
      }
      // Who said what about this item, and when. The machine's opinion is
      // never mixed into the content above it.
      const judgements = el('div', 'judgements');
      for (const j of d.judgements) {
        const line = el('div');
        line.append(el('b', null, `${j.level}`), document.createTextNode(
          ` · by ${j.by}${j.detail ? ' · ' + j.detail : ''} · ${j.at}`));
        judgements.append(line);
      }
      if (d.judgements.length) detail.append(judgements);
      // Ask for a reply. The drafter proposes; the approvals page decides.
      if (row.direction === 'inbound' && (row.connector === 'imap' || row.connector === 'telegram')) {
        const ask = el('button', 'choose', 'Draft a reply');
        ask.type = 'button';
        ask.addEventListener('click', async () => {
          ask.disabled = true;
          ask.textContent = 'drafting…';
          try {
            await post(`/api/item/${row.id}/draft`, {});
            ask.textContent = 'Draft ready in Approvals';
            loadApprovals();
          } catch (e) {
            ask.textContent = e.message;
          }
        });
        detail.append(ask);
      }
      // Your say. The marker in the row follows.
      const marker = li.querySelector('.level');
      detail.append(levelChooser(row.id, d.row.level, null, (updated) => {
        marker.replaceWith(levelMarker(updated.row.level, updated.row.level_reason));
        detail.remove();
        detail = null;
        li.classList.remove('is-open');
      }));
    } catch (e) {
      detail.replaceChildren(el('p', 'error', e.message));
    }
  });

  return li;
}

async function loadTimeline() {
  const params = new URLSearchParams({
    q: $('#q').value.trim(),
    level: $('#level').value,
    connector: $('#connector').value,
  });
  try {
    const rows = await get('/api/timeline?' + params);
    $('#rows').replaceChildren(...rows.map(rowNode));
    const empty = $('#timeline-empty');
    empty.hidden = rows.length > 0;
    empty.textContent = $('#q').value.trim()
      ? 'Nothing matches. Search needs at least three characters.'
      : 'Nothing yet. `genatrix seed` puts sample items in.';
  } catch (e) {
    $('#rows').replaceChildren();
    const empty = $('#timeline-empty');
    empty.hidden = false;
    empty.textContent = e.message;
    empty.classList.add('error');
  }
}

let typing;
$('#q').addEventListener('input', () => {
  clearTimeout(typing);
  typing = setTimeout(loadTimeline, 150);
});
$('#level').addEventListener('change', loadTimeline);
$('#connector').addEventListener('change', loadTimeline);
$('#filters').addEventListener('submit', (e) => e.preventDefault());

// --- today ------------------------------------------------------------

const GROUP_NAMES = {
  needs_reply: 'Needs your reply',
  promised: 'You promised',
  worth_knowing: 'Worth knowing',
};

// A source: who, when, where; click to read the item in place.
function sourceNode(src) {
  const link = el('button', 'source', `${src.who} · ${src.connector === 'imap' ? 'mail' : src.connector} · ${src.at}`);
  link.type = 'button';
  let open = null;
  link.addEventListener('click', async () => {
    if (open) { open.remove(); open = null; return; }
    open = el('div', 'detail');
    open.append(el('p', null, 'loading…'));
    link.parentElement.append(open);
    try {
      const d = await get(`/api/item/${src.id}`);
      open.replaceChildren();
      if (d.subject) open.append(el('p', null, d.subject));
      open.append(el('p', null, d.text));
    } catch (e) {
      open.replaceChildren(el('p', 'error', e.message));
    }
  });
  return link;
}

function pointNode(point) {
  const li = el('li', 'point' + (point.sources.length ? '' : ' unfounded'));
  li.append(el('span', 'text', point.text));
  if (point.sources.length) {
    const sources = el('div', 'sources');
    for (const s of point.sources) sources.append(sourceNode(s));
    li.append(sources);
  } else {
    li.append(el('span', 'note', 'no source'));
  }
  return li;
}

function commitmentNode(c) {
  const li = el('li', 'row is-open commitment ' + c.status);
  const meta = el('div', 'meta');
  meta.append(
    el('span', 'who', c.mine ? 'You' : c.from),
    el('span', null, c.to ? `→ ${c.mine ? c.to : 'you'}` : ''),
    el('span', 'thread', c.due ? `by ${c.due}` : 'no date'),
    el('span', 'standing', c.standing === 'confirmed' ? 'confirmed' : 'inferred'),
    el('span', c.status === 'overdue' ? 'error' : null, c.status === 'overdue' ? 'overdue' : ''),
  );
  li.append(meta, el('p', 'preview', c.what));
  const sources = el('div', 'sources');
  for (const s of c.evidence) sources.append(sourceNode(s));
  li.append(sources);
  const box = el('div', 'chooser');
  const act = (label, standing, status) => {
    const b = el('button', 'choose', label);
    b.type = 'button';
    b.addEventListener('click', async () => {
      for (const x of box.querySelectorAll('button')) x.disabled = true;
      try { await post(`/api/commitment/${c.id}`, { standing, status }); li.remove(); }
      catch (e) { box.append(el('span', 'error', e.message)); }
    });
    return b;
  };
  if (c.standing !== 'confirmed') box.append(act('Yes, I did promise this', 'confirmed', null));
  box.append(act('Done', 'confirmed', 'done'));
  box.append(act('Not a promise', 'rejected', 'cancelled'));
  li.append(box);
  return li;
}

async function loadToday() {
  try {
    const t = await get('/api/today');
    const box = $('#digest');
    box.replaceChildren();
    if (!t.digest) {
      $('#digest-headline').textContent = 'No digest yet. The first one is made after eight in the morning, once the model side is up.';
    } else {
      $('#digest-headline').textContent = `Digest for ${t.digest.day}, made ${t.digest.generated_at} from ${t.digest.considered} messages`;
      for (const g of t.digest.groups) {
        if (g.group === 'promised') continue; // shown live below, from the commitments themselves
        box.append(el('h2', 'group-title', `${GROUP_NAMES[g.group]} (${g.points.length})`));
        const ol = el('ol', 'points');
        if (!g.points.length) ol.append(el('li', 'point empty', 'nothing'));
        for (const p of g.points) ol.append(pointNode(p));
        box.append(ol);
      }
    }
    $('#today-actions').replaceChildren(...t.actions.map(a => actionCard(a, loadToday)));
    const noActions = $('#today-actions-empty');
    noActions.hidden = t.actions.length > 0;
    noActions.textContent = 'Nothing waiting for your approval.';
    updateBadge(t.pending_actions);
    $('#commitments').replaceChildren(...t.commitments.map(commitmentNode));
    const empty = $('#commitments-empty');
    empty.hidden = t.commitments.length > 0;
    empty.textContent = 'No open promises found in your messages.';
  } catch (e) {
    $('#digest-headline').textContent = e.message;
    $('#digest-headline').classList.add('error');
  }
}

// --- approvals --------------------------------------------------------
//
// Design 06: the one screen where principle five is kept. Evidence before
// the draft; the draft editable in place; the approve button names the
// consequence; a decline wants a reason; what you approve is the version
// you see, checked by hash and by the nonce this page was handed.

const KIND_WORDS = { send_mail: 'Reply by mail', send_message: 'Reply on Telegram', create_event: 'Add an event', write_memory: 'Remember' };

function expiresIn(secs) {
  if (secs <= 0) return 'expired';
  const d = Math.floor(secs / 86400), h = Math.floor((secs % 86400) / 3600);
  if (d >= 1) return `expires in ${d} day${d === 1 ? '' : 's'}`;
  if (h >= 1) return `expires in ${h} hour${h === 1 ? '' : 's'}`;
  return 'expires within the hour';
}

function actionCard(a, onDone) {
  const li = el('li', 'row is-open action ' + a.status);
  const head = el('div', 'meta');
  head.append(
    el('span', 'who', `${KIND_WORDS[a.kind] || a.kind} → ${a.target}`),
    el('span', 'faint', a.status === 'pending' ? expiresIn(a.expires_in_secs) : a.status + (a.status_detail ? ` · ${a.status_detail}` : '')),
  );
  li.append(head);

  if (a.evidence.length) {
    li.append(el('h4', 'card-label', 'Evidence'));
    const sources = el('div', 'sources');
    for (const s of a.evidence) sources.append(sourceNode(s));
    li.append(sources);
  }
  if (a.rationale) {
    li.append(el('h4', 'card-label', 'Why'));
    li.append(el('p', 'rationale', a.rationale));
  }
  li.append(el('h4', 'card-label', `Draft${a.versions > 1 ? ` · version ${a.version}` : ''}`));
  const draft = el('textarea', 'draft');
  draft.value = a.draft;
  draft.readOnly = a.status !== 'pending';
  li.append(draft);
  li.append(el('p', 'note', `Drafted ${a.drafted}. From ${a.account}.`));

  if (a.status !== 'pending') return li;

  let current = a; // the version and nonce this card holds
  const box = el('div', 'chooser');
  const msg = el('span', 'note');
  const approve = el('button', 'choose primary', 'Approve and send');
  approve.type = 'button';
  const decline = el('button', 'choose lowers', 'Decline…');
  decline.type = 'button';
  const reason = el('input', 'reason');
  reason.placeholder = 'why? a word or two';
  reason.hidden = true;

  const busy = (on) => { for (const b of box.querySelectorAll('button')) b.disabled = on; };
  const saveEditIfAny = async () => {
    if (draft.value !== current.draft) {
      current = await post(`/api/action/${a.id}/edit`, { payload: draft.value });
      draft.value = current.draft;
    }
  };
  approve.addEventListener('click', async () => {
    busy(true);
    try {
      await saveEditIfAny();
      const done = await post(`/api/action/${a.id}/approve`, {
        version: current.version, payload_hash: current.payload_hash, nonce: current.nonce,
      });
      li.className = 'row is-open action ' + done.status;
      box.replaceChildren(el('span', 'note', 'Approved. The connector sends it and reports back here.'));
      onDone && onDone();
    } catch (e) { msg.textContent = e.message; msg.classList.add('error'); busy(false); }
  });
  decline.addEventListener('click', async () => {
    if (reason.hidden) { reason.hidden = false; reason.focus(); return; }
    busy(true);
    try {
      const done = await post(`/api/action/${a.id}/decline`, { reason: reason.value });
      li.className = 'row is-open action ' + done.status;
      box.replaceChildren(el('span', 'note', `Declined: ${done.status_detail}`));
      onDone && onDone();
    } catch (e) { msg.textContent = e.message; msg.classList.add('error'); busy(false); }
  });
  reason.addEventListener('keydown', (e) => { if (e.key === 'Enter') decline.click(); });
  box.append(approve, decline, reason, msg);
  li.append(box);
  return li;
}

async function loadApprovals() {
  try {
    const pending = await get('/api/actions?status=pending&limit=100');
    $('#approvals-pending').replaceChildren(...pending.map(a => actionCard(a, loadApprovals)));
    const empty = $('#approvals-empty');
    empty.hidden = pending.length > 0;
    empty.textContent = 'Nothing waiting. A draft appears here when you ask for one from a message, or when the digest proposes a reply.';
    const all = await get('/api/actions?limit=60');
    $('#approvals-history').replaceChildren(...all.filter(a => a.status !== 'pending').map(a => actionCard(a)));
    updateBadge(pending.length);
  } catch (e) {
    $('#approvals-empty').hidden = false;
    $('#approvals-empty').textContent = e.message;
  }
}

function updateBadge(n) {
  const badge = $('#pending-badge');
  badge.hidden = !n;
  badge.textContent = n;
}

// --- people -----------------------------------------------------------
//
// Design 07: a relationship is the person, the roles you gave them, the
// notes you wrote, and arithmetic over what passed between you. Nothing on
// this page is the model's opinion.

const HANDLE_ICONS = { email: '✉', telegramid: '✈', telegramusername: '@', phone: '☏' };

function initials(name) {
  const clean = (name || '').trim();
  if (!clean) return '?';
  const parts = clean.split(/\s+/);
  const first = [...parts[0]][0] || '?';
  const second = parts.length > 1 ? [...parts[parts.length - 1]][0] : '';
  return (first + second).toUpperCase();
}

// A tiny bar chart: the last twelve months, the busiest month full height.
function monthBars(months) {
  const box = el('div', 'months');
  const max = Math.max(1, ...months);
  months.forEach((n, i) => {
    const bar = el('span', 'month' + (i === months.length - 1 ? ' now' : ''));
    bar.style.height = `${Math.max(2, Math.round(n / max * 28))}px`;
    bar.title = `${n} message${n === 1 ? '' : 's'}`;
    box.append(bar);
  });
  return box;
}

function personCard(card, onOpen) {
  const li = el('li', 'person-card');
  li.dataset.id = card.id;
  const avatar = el('span', 'avatar', initials(card.name));
  const body = el('div', 'person-body');
  const head = el('div', 'person-head');
  head.append(el('span', 'person-name', card.name));
  for (const r of card.roles) head.append(el('span', 'role', r));
  body.append(head);
  const line = el('div', 'person-line');
  line.append(
    el('span', 'count in', `↓ ${card.from_them}`),
    el('span', 'count out', `↑ ${card.to_them}`),
    el('span', 'faint', card.last_at ? `last ${card.last_at}` : ''),
    el('span', 'faint', card.connectors.map(c => c === 'imap' ? 'mail' : c).join(' · ')),
  );
  body.append(line);
  li.append(avatar, body);
  li.addEventListener('click', () => onOpen(card.id, li));
  return li;
}

function statTile(label, value, note) {
  const tile = el('div', 'tile');
  tile.append(el('span', 'tile-value', value), el('span', 'tile-label', label));
  if (note) tile.append(el('span', 'tile-note', note));
  return tile;
}

function hours(h) {
  if (h == null) return '—';
  if (h < 1) return `${Math.round(h * 60)} min`;
  if (h < 48) return `${h.toFixed(1)} h`;
  return `${(h / 24).toFixed(1)} d`;
}

async function openPerson(id) {
  const box = $('#person');
  box.replaceChildren(el('p', 'empty', 'loading…'));
  for (const c of document.querySelectorAll('.person-card')) c.classList.toggle('is-on', c.dataset.id === id);
  try {
    const d = await get(`/api/person/${id}`);
    box.replaceChildren();

    // Head: who, with the roles you gave them; click a role to remove it,
    // type to add one.
    const head = el('div', 'person-detail-head');
    head.append(el('span', 'avatar big', initials(d.card.name)));
    const title = el('div');
    title.append(el('h2', 'person-title', d.card.name));
    const roles = el('div', 'roles');
    const saveRelationship = async () => {
      const current = [...roles.querySelectorAll('.role')].map(r => r.dataset.role);
      await post(`/api/person/${id}/relationship`, { roles: current, notes: notes.value });
    };
    const addRole = (name) => {
      const chip = el('span', 'role removable', name);
      chip.dataset.role = name;
      chip.title = 'remove';
      chip.addEventListener('click', async () => { chip.remove(); await saveRelationship(); });
      roles.insertBefore(chip, input);
    };
    const input = el('input', 'role-input');
    input.placeholder = 'add a role…';
    input.addEventListener('keydown', async (e) => {
      if (e.key === 'Enter' && input.value.trim()) {
        addRole(input.value.trim());
        input.value = '';
        await saveRelationship();
      }
    });
    roles.append(input);
    for (const r of d.card.roles) addRole(r);
    title.append(roles);
    const handles = el('div', 'handles');
    for (const h of d.card.handles) {
      handles.append(el('span', 'handle' + (h.inferred ? ' inferred' : ''),
        `${HANDLE_ICONS[h.kind] || ''} ${h.value}`));
    }
    title.append(handles);
    head.append(title);
    box.append(head);

    // The arithmetic.
    const tiles = el('div', 'tiles');
    tiles.append(
      statTile('from them', d.stats.from_them),
      statTile('from you', d.stats.to_them),
      statTile('your reply time', hours(d.stats.reply_hours), d.stats.reply_hours == null ? 'no exchanges to measure' : 'median'),
      statTile('since', d.stats.first_at || '—', d.stats.last_at ? `last ${d.stats.last_at}` : ''),
      statTile('language', d.stats.language || '—'),
    );
    box.append(tiles);
    const chart = el('div', 'chart');
    chart.append(el('span', 'chart-label', 'last twelve months'), monthBars(d.stats.months));
    box.append(chart);

    // Your notes.
    box.append(el('h3', 'group-title', 'Your notes'));
    const notes = el('textarea', 'notes');
    notes.value = d.notes || '';
    notes.placeholder = 'Anything you want to remember about them. Only you write here.';
    notes.addEventListener('blur', saveRelationship);
    box.append(notes);

    // Promises either way.
    if (d.commitments.length) {
      box.append(el('h3', 'group-title', `Promises (${d.commitments.length})`));
      const ol = el('ol', 'rows');
      for (const c of d.commitments) ol.append(commitmentNode(c));
      box.append(ol);
    }

    // Recent messages, the same rows as the timeline.
    box.append(el('h3', 'group-title', 'Recent'));
    const ol = el('ol', 'rows');
    for (const r of d.recent) ol.append(rowNode(r));
    if (!d.recent.length) ol.append(el('li', 'empty', 'nothing yet'));
    box.append(ol);
  } catch (e) {
    box.replaceChildren(el('p', 'error', e.message));
  }
}

async function loadPeople() {
  const list = $('#people-list');
  try {
    const cards = await get('/api/people?limit=80');
    list.replaceChildren(...cards.map(c => personCard(c, openPerson)));
    if (!cards.length) list.append(el('li', 'empty', 'No one yet.'));
  } catch (e) {
    list.replaceChildren(el('li', 'empty error', e.message));
  }
}

// --- review -----------------------------------------------------------

function reviewNode(entry) {
  const li = el('li', 'row is-open');
  const meta = el('div', 'meta');
  meta.append(
    el('time', null, entry.row.at),
    levelMarker(entry.row.level, entry.row.level_reason),
    el('span', 'who', entry.row.author),
  );
  li.append(meta);
  if (entry.subject) li.append(el('p', 'preview', entry.subject));
  li.append(el('p', 'excerpt', entry.text));
  li.append(el('p', 'note', `The model said ${entry.model_level}.`));
  li.append(levelChooser(entry.row.id, entry.row.level, entry.model_level, () => {
    li.remove();
    loadTally();
    if (!$('#review-rows').children.length) loadReview();
  }));
  return li;
}

async function loadTally() {
  try {
    const view = await get('/api/review?count=0');
    const t = view.tally;
    const rate = t.reviewed ? Math.round(100 * t.agreed / t.reviewed) : null;
    $('#tally').textContent = t.judged === 0
      ? 'The model has not judged anything yet.'
      : `${t.judged} judged by the model · ${t.reviewed} reviewed by you` +
        (rate === null ? '' : ` · agreed ${t.agreed} of ${t.reviewed} (${rate}%)`);
  } catch (e) {
    $('#tally').textContent = e.message;
  }
}

async function loadReview() {
  await loadTally();
  try {
    const view = await get('/api/review?count=20');
    $('#review-rows').replaceChildren(...view.items.map(reviewNode));
    const empty = $('#review-empty');
    empty.hidden = view.items.length > 0;
    empty.textContent = view.tally.judged === 0
      ? 'Nothing to review until the model has judged some mail.'
      : 'You have looked at everything the model judged.';
  } catch (e) {
    $('#review-rows').replaceChildren();
    const empty = $('#review-empty');
    empty.hidden = false;
    empty.textContent = e.message;
  }
}
$('#review-more').addEventListener('click', loadReview);

// --- records ----------------------------------------------------------

function callNode(call) {
  const li = el('li', 'row');
  const meta = el('div', 'meta');
  meta.append(
    el('time', null, call.at),
    levelMarker(call.level, ''),
    el('span', 'who', call.purpose),
    el('span', null, call.location === 'cloud' ? `${call.target} · cloud` : call.target),
    el('span', 'thread', `${call.items} item${call.items === 1 ? '' : 's'}`),
  );
  li.append(meta);
  if (call.payload) {
    li.append(el('pre', 'payload', call.payload));
  } else {
    li.append(el('p', 'preview', 'Answered on this device; nothing was sent.'));
  }
  return li;
}

async function loadRecords() {
  try {
    const view = await get('/api/ledger');
    $('#headline').textContent = view.headline;
    $('#chain').textContent =
      `${view.entries} entries, ${view.verified ? 'chain verified to the root' : 'CHAIN BROKEN'}`;
    $('#chain').classList.toggle('error', !view.verified);
    $('#calls').replaceChildren(...view.calls.map(callNode));
    const empty = $('#records-empty');
    empty.hidden = view.calls.length > 0;
    empty.textContent = 'No model calls recorded yet.';
  } catch (e) {
    $('#headline').textContent = e.message;
    $('#headline').classList.add('error');
  }
}

loadStatus();
setInterval(loadStatus, 5000);
loadToday();
