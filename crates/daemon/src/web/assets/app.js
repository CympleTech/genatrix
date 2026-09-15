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
  });
}

// --- status -----------------------------------------------------------

async function loadStatus() {
  try {
    const s = await get('/api/status');
    const bytes = s.bytes_left_device === 0
      ? 'nothing has left this device'
      : `${s.bytes_left_device} bytes have left this device`;
    $('#status').textContent =
      `${s.items} items · cloud ${s.cloud_enabled ? 'on' : 'off'} · ${bytes}`;
  } catch (e) {
    $('#status').textContent = e.message;
    $('#status').classList.add('error');
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
loadTimeline();
