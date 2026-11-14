<script lang="ts">
  // Design 15: the trusted shell. It is the top-level page; the interface
  // runs inside it, in a sandboxed frame with no origin, no credentials and
  // no way out but this page. Everything the interface asks of the core
  // comes here first, and the gate (lib/gate.ts) says what happens to it.
  // Approvals, the passphrase, pairing, devices, export and erase, and
  // which interface runs are drawn here, never by the interface.
  import { get, post, ApiError, type Action } from './lib/api';
  import { t } from './lib/i18n';
  import { route, go } from './lib/router';
  import { judge, shown, type Verdict } from './lib/gate';
  import { packForFrame, keepForFrame } from './lib/storage';
  import ActionCard from './components/ActionCard.svelte';
  import PassphraseDialog from './components/PassphraseDialog.svelte';
  import SecurityPanel from './components/SecurityPanel.svelte';
  import InterfacePanel from './components/InterfacePanel.svelte';
  import Pair from './pages/Pair.svelte';
  import ChatBubble from './components/ChatBubble.svelte';

  interface UiState {
    device: string; using: string; load: string; default_hash: string; chosen: boolean; others: number;
    uis: { name: string; title: string; project?: string | null }[];
  }

  let ui = $state<UiState | null>(null);
  let stranger = $state(false);
  let failed = $state('');
  let reloads = $state(0);
  let frame = $state<HTMLIFrameElement | null>(null);

  async function loadUi() {
    try { ui = await get<UiState>('/api/ui'); stranger = false; failed = ''; }
    catch (e: any) {
      if (e instanceof ApiError && e.status === 401) stranger = true;
      else failed = e.message;
    }
  }
  // Asked when the page opens, and again after pairing.
  $effect(() => { if (!pairing && !ui) loadUi(); });

  // Where the frame is: the address bar's path, without the shell's own
  // words in it.
  const safe = $derived($route.parts[0] === 'safe');
  const pairing = $derived($route.parts[0] === 'pair');
  const preview = $derived($route.query.get('preview'));
  function framePath(): string {
    const q = new URLSearchParams(location.search);
    q.delete('preview');
    const rest = q.toString();
    const path = location.pathname.replace(/^\/safe(?=\/|$)/, '') || '/';
    return path + (rest ? '?' + rest : '');
  }
  const hash = $derived(!ui ? null : safe ? ui.default_hash : preview && /^[0-9a-f]{64}$/.test(preview) ? preview : ui.load);
  // The frame is made again only when what it runs changes, or it asks to
  // be shown again; moving between pages is a message, not a reload.
  const src = $derived.by(() => {
    reloads;
    if (!hash) return '';
    return `/ui/${hash}/index.html?s=${encodeURIComponent(packForFrame())}#${encodeURIComponent(framePath())}`;
  });

  // What the frame last said about where it is, so it is not told back.
  let fromFrame = '';
  // The frame's path is the address without the shell's own words in it:
  // `/safe` in front, `preview=` in the query.
  function inFrame(path: string, query: URLSearchParams): string {
    const q = new URLSearchParams(query);
    q.delete('preview');
    const p = path.replace(/^\/safe(?=\/|$)/, '') || '/';
    return p + (q.toString() ? '?' + q.toString() : '');
  }
  $effect(() => {
    const p = inFrame($route.path, $route.query);
    if (p === fromFrame) return;
    frame?.contentWindow?.postMessage({ t: 'route', path: p }, '*');
  });

  // --- the sheets the shell draws ---
  interface Ask { verdict: Verdict; method: string; path: string; body: unknown; resolve: (yes: boolean) => void }
  let asking = $state<Ask | null>(null);
  let reviewing = $state<{ action: Action | null; error: string; resolve: (a: Action | null) => void } | null>(null);
  let opening = $state<{ url: string; download: boolean } | null>(null);
  let panel = $state<'' | 'security' | 'interface'>('');
  let refused = $state('');

  function confirmFirst(verdict: Verdict, method: string, path: string, body: unknown): Promise<boolean> {
    return new Promise((resolve) => { asking = { verdict, method, path, body, resolve }; });
  }
  function answerAsk(yes: boolean) { const a = asking; asking = null; a?.resolve(yes); }

  async function openReview(id: string): Promise<Action | null> {
    return new Promise((resolve) => {
      reviewing = { action: null, error: '', resolve };
      get<Action>(`/api/action/${encodeURIComponent(id)}`)
        .then((a) => { if (reviewing) reviewing.action = a; })
        .catch((e: any) => { if (reviewing) reviewing.error = e.message; });
    });
  }
  async function closeReview() {
    const r = reviewing; reviewing = null;
    if (!r) return;
    let now: Action | null = r.action;
    if (now) { try { now = await get<Action>(`/api/action/${encodeURIComponent(now.id)}`); } catch { /* as it was */ } }
    r.resolve(now);
  }

  async function forward(method: string, path: string, body: unknown): Promise<{ ok: boolean; status: number; body: any }> {
    try {
      const out = method === 'GET' ? await get(path) : await post(path, body ?? {});
      return { ok: true, status: 200, body: out };
    } catch (e: any) {
      if (e instanceof ApiError) return { ok: false, status: e.status, body: e.body ?? { error: e.message } };
      return { ok: false, status: 0, body: { error: e?.message ?? String(e) } };
    }
  }

  async function handleCall(m: any): Promise<{ ok: boolean; status: number; body: any }> {
    const method = String(m.method ?? 'GET').toUpperCase();
    const path = String(m.path ?? '');
    const verdict = judge(method, path);
    switch (verdict.way) {
      case 'pass':
        return forward(method, path, m.body);
      case 'confirm': {
        if (!(await confirmFirst(verdict, method, path, m.body))) return { ok: false, status: 409, body: { error: t('shell.declined') } };
        return forward(method, path, m.body);
      }
      case 'review': {
        const a = await openReview(verdict.target ?? '');
        return a ? { ok: true, status: 200, body: a } : { ok: false, status: 409, body: { error: t('shell.declined') } };
      }
      default:
        refused = t('shell.refused');
        return { ok: false, status: 403, body: { error: t('shell.refused') } };
    }
  }

  function reply(id: unknown, a: { ok: boolean; status: number; body: any }) {
    frame?.contentWindow?.postMessage({ t: 'result', id, ...a }, '*');
  }

  // Only the frame this page made is listened to.
  function onMessage(e: MessageEvent) {
    if (!frame || e.source !== frame.contentWindow) return;
    const m = e.data;
    if (!m || typeof m !== 'object' || typeof m.t !== 'string') return;
    switch (m.t) {
      case 'call':
        handleCall(m).then((a) => reply(m.id, a));
        break;
      case 'review':
        openReview(String(m.action ?? '')).then((a) => reply(m.id, a ? { ok: true, status: 200, body: a } : { ok: false, status: 409, body: null }));
        break;
      case 'nav': {
        const p = String(m.path ?? '/');
        if (!p.startsWith('/') || p.startsWith('//')) break;
        if (p.startsWith('/pair')) { go(p); break; }
        fromFrame = p;
        // The shell's own words stay in the address as the frame moves.
        const u = new URL(p, location.origin);
        if (preview) u.searchParams.set('preview', preview);
        const there = u.pathname + (u.search || '');
        go(safe ? '/safe' + (there === '/' ? '' : there) : there, !!m.replace);
        break;
      }
      case 'open': {
        const url = String(m.url ?? '');
        if (url.startsWith('/api/') || /^https?:\/\//.test(url)) opening = { url, download: !!m.download };
        break;
      }
      case 'panel':
        if (m.panel === 'security' || m.panel === 'interface') panel = m.panel;
        else if (m.panel === 'pair') go('/pair');
        break;
      case 'store':
        keepForFrame(m.key, m.value);
        break;
      case 'reload':
        reloads++;
        break;
    }
  }
  $effect(() => {
    window.addEventListener('message', onMessage);
    return () => window.removeEventListener('message', onMessage);
  });

  function openLink() {
    const o = opening; opening = null;
    if (!o) return;
    if (o.url.startsWith('/api/')) {
      const a = document.createElement('a');
      a.href = o.url; if (o.download) a.download = '';
      document.body.appendChild(a); a.click(); a.remove();
    } else {
      window.open(o.url, '_blank', 'noopener,noreferrer');
    }
  }
  const what = (v: Verdict) => { const k = `shell.ask.${v.what}`; const w = t(k); return w === k ? t('shell.ask.unknown') : w; };
  // What the floating chat needs to know of the interface in use.
  const usingInfo = $derived.by(() => {
    const u = ui?.uis.find((x) => x.name === ui?.using);
    return { name: ui?.using ?? 'default', title: u?.title ?? '', project: !!u?.project };
  });
  // A new interface for this device: asked again and loaded in the frame;
  // the shell, and the chat with it, stay as they are.
  async function chosen() { panel = ''; await loadUi(); reloads++; }
  // A device that has not picked an interface, when there is more than the
  // default to pick from, is asked once (design 15, "每台设备选一个界面").
  let offered = false;
  $effect(() => {
    if (ui && !ui.chosen && ui.others > 0 && !offered && !safe) { offered = true; panel = 'interface'; }
  });
</script>

{#if pairing}
  <div class="shell-page"><Pair /></div>
{:else if stranger}
  <div class="shell-page">
    <section class="pane">
      <p class="headline">{t('unauthorized')}</p>
      <p class="note">{t('unauthorized.hint')}</p>
      <button type="button" class="choose primary" onclick={() => go('/pair')}>{t('pair.title')}</button>
    </section>
  </div>
{:else if failed && !ui}
  <div class="shell-page"><section class="pane"><p class="note error">{failed}</p>
    <button type="button" class="choose" onclick={loadUi}>{t('shell.retry')}</button></section></div>
{:else}
  {#if safe || preview}
    <div class="shell-band" role="status">
      <span>{safe ? t('shell.safe') : t('shell.preview')}</span>
      <button type="button" class="linkish" onclick={() => (panel = 'interface')}>{t('shell.interface.open')}</button>
      <button type="button" class="linkish" onclick={() => { location.href = '/'; }}>{t('shell.leave')}</button>
    </div>
  {/if}
  {#if src}
    <iframe bind:this={frame} class="ui-frame" class:banded={safe || !!preview} title="Genatrix" {src}
      sandbox="allow-scripts allow-forms"></iframe>
  {/if}
{/if}

{#if refused}
  <div class="shell-toast" role="alert">
    <span>{refused}</span>
    <button type="button" class="linkish" onclick={() => { refused = ''; panel = 'security'; }}>{t('shell.security.open')}</button>
    <button type="button" class="linkish" onclick={() => (refused = '')}>{t('admin.news.ok')}</button>
  </div>
{/if}

{#if asking}
  <div class="passphrase-scrim" role="presentation"></div>
  <div class="passphrase-sheet shell-sheet" role="dialog" aria-modal="true" aria-labelledby="shell-ask-title">
    <h2 id="shell-ask-title" class="group-title">{t('shell.ask.title')}</h2>
    <p class="headline small">{what(asking.verdict)}</p>
    <p class="note">{t('shell.ask.note')}</p>
    <dl class="card-fields">
      <dt>{t('shell.ask.request')}</dt><dd><code>{asking.method} {asking.path}</code></dd>
      {#each shown(asking.body) as [k, v]}<dt>{k}</dt><dd>{v}</dd>{/each}
    </dl>
    <div class="chooser">
      <button type="button" class="choose primary" onclick={() => answerAsk(true)}>{t('shell.ask.yes')}</button>
      <button type="button" class="choose lowers" onclick={() => answerAsk(false)}>{t('accounts.cancel')}</button>
    </div>
  </div>
{/if}

{#if reviewing}
  <div class="passphrase-scrim" role="presentation"></div>
  <div class="passphrase-sheet shell-sheet review-sheet" role="dialog" aria-modal="true" aria-labelledby="shell-review-title">
    <h2 id="shell-review-title" class="group-title">{t('shell.review.title')}</h2>
    <p class="note">{t('shell.review.note')}</p>
    {#if reviewing.error}<p class="note error">{reviewing.error}</p>
    {:else if !reviewing.action}<p class="note">{t('loading')}</p>
    {:else}
      <ol class="rows cards">{#key reviewing.action.id}<ActionCard action={reviewing.action} ondone={() => {}} />{/key}</ol>
    {/if}
    <div class="chooser"><button type="button" class="choose lowers" onclick={closeReview}>{t('shell.close')}</button></div>
  </div>
{/if}

{#if opening}
  <div class="passphrase-scrim" role="presentation" onclick={() => (opening = null)}></div>
  <div class="passphrase-sheet shell-sheet" role="dialog" aria-modal="true" aria-labelledby="shell-open-title">
    <h2 id="shell-open-title" class="group-title">{opening.url.startsWith('/api/') ? t('shell.open.download') : t('shell.open.title')}</h2>
    <p class="note">{t('shell.open.note')}</p>
    <p class="open-url">{opening.url}</p>
    <div class="chooser">
      <button type="button" class="choose primary" onclick={openLink}>{t('shell.open.go')}</button>
      <button type="button" class="choose lowers" onclick={() => (opening = null)}>{t('accounts.cancel')}</button>
    </div>
  </div>
{/if}

{#if panel}
  <div class="passphrase-scrim" role="presentation" onclick={() => (panel = '')}></div>
  <div class="shell-panel" role="dialog" aria-modal="true">
    <div class="shell-panel-head">
      <h2 class="group-title">{panel === 'security' ? t('shell.security.title') : t('shell.interface.title')}</h2>
      <button type="button" class="choose lowers" onclick={() => (panel = '')}>{t('shell.close')}</button>
    </div>
    {#if panel === 'security'}<SecurityPanel />{:else}<InterfacePanel onchosen={chosen} onclose={() => (panel = '')} onreview={(id) => { panel = ''; openReview(id); }} />{/if}
  </div>
{/if}

{#if ui && !pairing && !stranger}
  <ChatBubble using={usingInfo} onreview={openReview} onchanged={chosen} />
{/if}

<PassphraseDialog />
