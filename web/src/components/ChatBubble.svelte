<script lang="ts">
  // Design 15 v0.4, "悬浮对话框": the shell's floating AI chat. Ask about
  // your data or the page you are on (the local model, as on Ask); or ask
  // for this page to change (a work card on the interface's source, decided
  // in the shell, followed here until its build waits for your approval).
  // The conversation lives while the page is open and nowhere else
  // (design 03: a conversation's context is not memory).
  import { tick } from 'svelte';
  import { get, post, type Action } from '../lib/api';
  import { t } from '../lib/i18n';
  import { route, go } from '../lib/router';

  let { using, onreview, onchanged }: {
    using: { name: string; title: string; project: boolean };
    onreview: (action: string) => Promise<Action | null>;
    onchanged: () => void;
  } = $props();

  type Mode = 'ask' | 'change';
  interface Button { label: string; run: () => void; primary?: boolean }
  interface Msg { who: 'me' | 'ai' | 'note'; text: string; buttons?: Button[]; error?: boolean; work?: string }

  let open = $state(false);
  let mode = $state<Mode>('ask');
  let msgs = $state<Msg[]>([]);
  let text = $state('');
  let busy = $state(false);
  let list = $state<HTMLElement | null>(null);
  let field = $state<HTMLTextAreaElement | null>(null);

  // Where you are, in words: the page's own name and its address.
  const PAGES: Record<string, string> = {
    '': 'today', approvals: 'approvals', chats: 'chats', ask: 'ask', spaces: 'spaces', work: 'spaces',
    timeline: 'timeline', records: 'records', review: 'review', settings: 'settings', welcome: 'welcome.title',
  };
  const path = $derived(($route.path.replace(/^\/safe(?=\/|$)/, '') || '/') + ($route.query.toString() ? '?' + new URLSearchParams([...$route.query].filter(([k]) => k !== 'preview')).toString() : ''));
  const pageName = $derived.by(() => {
    const first = path.split('/').filter(Boolean)[0]?.split('?')[0] ?? '';
    const key = PAGES[first];
    return key ? t(key) : path;
  });
  const where = $derived(`${pageName} (${path})`);

  async function down() {
    await tick();
    if (list) list.scrollTop = list.scrollHeight;
  }
  function say(m: Msg): Msg {
    msgs.push(m);
    down();
    return msgs[msgs.length - 1];
  }
  $effect(() => { if (open) queueMicrotask(() => field?.focus()); });

  async function send(e?: Event) {
    e?.preventDefault();
    const q = text.trim();
    if (!q || busy) return;
    text = '';
    say({ who: 'me', text: q });
    // Asking the model to change the page gets a made-up answer: it can
    // change nothing. A sentence that sounds like a change is asked about
    // first, and goes where it can be done.
    if (mode === 'ask' && looksLikeChange(q)) {
      say({
        who: 'ai', text: t('chat.bubble.sounds'),
        buttons: [
          { label: t('chat.bubble.change'), primary: true, run: () => { mode = 'change'; run(() => change(q)); } },
          { label: t('chat.bubble.justask'), run: () => run(() => ask(q)) },
        ],
      });
      return;
    }
    busy = true;
    try {
      if (mode === 'ask') await ask(q);
      else await change(q);
    } catch (err: any) {
      say({ who: 'ai', text: err.message, error: true });
    }
    busy = false;
    queueMicrotask(() => field?.focus());
  }

  // Words that ask for the page itself to be different.
  const CHANGE_WORDS = /(改成|改为|改一下|修改|调整|调成|换成|挪到|移到|放到|放在|加一个|加个|增加|去掉|删掉|隐藏|默认|按钮|布局|排版|颜色|字体|字号|大一点|小一点|宽一点|窄一点|排序|折叠|展开|点开|只显示|显示全部|不显示)/;
  const CHANGE_WORDS_EN = /\b(change|move|make it|show only|hide|by default|button|layout|colou?r|font|bigger|smaller|wider|narrower|collapse|expand|put the|add a|remove the|sort)\b/i;
  function looksLikeChange(q: string): boolean {
    return CHANGE_WORDS.test(q) || CHANGE_WORDS_EN.test(q);
  }
  async function run(fn: () => Promise<void>) {
    busy = true;
    try { await fn(); } catch (err: any) { say({ who: 'ai', text: err.message, error: true }); }
    busy = false;
  }

  // --- asking ---
  async function ask(q: string) {
    const history = msgs
      .filter((m, i) => m.who === 'me' && msgs[i + 1]?.who === 'ai' && !msgs[i + 1].error && !msgs[i + 1].work)
      .slice(-6)
      .map((m) => ({ user: m.text, answer: msgs[msgs.indexOf(m) + 1].text }));
    const r = await post<{ answer: string; actions: unknown[] }>('/api/ask', {
      question: `${q}\n\n(${t('chat.bubble.context', { page: where })})`,
      history,
    });
    say({ who: 'ai', text: r.answer });
    if (r.actions.length) say({ who: 'note', text: t('chat.bubble.proposed') });
  }

  // --- changing this page ---
  async function makeChangeable() {
    busy = true;
    try {
      const r = await post<{ name: string }>('/api/ui/copy', { from: 'default', title: t('chat.bubble.mine') });
      await post('/api/ui/choose', { ui: r.name });
      say({ who: 'note', text: t('chat.bubble.copied') });
      onchanged();
    } catch (err: any) { say({ who: 'ai', text: err.message, error: true }); }
    busy = false;
  }

  async function change(q: string) {
    if (!using.project) {
      say({ who: 'ai', text: t('chat.bubble.nosource'), buttons: [{ label: t('chat.bubble.copy'), primary: true, run: makeChangeable }] });
      return;
    }
    // Where you asked from: the page comes back here to show the result.
    const from = path;
    const r = await post<{ action: string }>('/api/ui/change', { ui: using.name, task: q, page: where });
    say({ who: 'note', text: t('chat.bubble.drafted') });
    const decided = await onreview(r.action);
    if (!decided || decided.status === 'pending') {
      say({ who: 'note', text: t('chat.bubble.waiting'), buttons: [{ label: t('action.review'), run: () => decide(r.action, from, q) }] });
      return;
    }
    if (decided.status !== 'approved' && decided.status !== 'executed') {
      say({ who: 'note', text: t('chat.bubble.declined') });
      return;
    }
    started(r.action, from, q);
  }

  async function decide(id: string, from: string, q: string) {
    const a = await onreview(id);
    if (a && (a.status === 'approved' || a.status === 'executed')) started(id, from, q);
  }

  // Design 15 v0.5: once approved, the page shows the work going on; when
  // it is done, the page goes back to where you asked and shows the result.
  function started(id: string, from: string, q: string) {
    go('/work/' + id);
    follow(id, from, q);
  }

  // The address of a page, previewing a version.
  function previewing(at: string, hash: string): string {
    const u = new URL(at, location.origin);
    u.searchParams.set('preview', hash);
    return u.pathname + u.search;
  }

  // Follow one piece of work until it ends, answering its questions here.
  interface WorkView { dispatch: { state: string; detail: string | null; started_ms: number }; question: { seq: number; question: string; options: string[]; recommend: number | null } | null }
  async function follow(id: string, from: string, asked: string) {
    const line = say({ who: 'ai', text: t('chat.bubble.working', { min: 0 }), work: id, buttons: [{ label: t('chat.bubble.watch'), run: () => go('/work/' + id) }] });
    let lastQuestion = -1;
    for (let i = 0; i < 2400; i++) {
      await new Promise((r) => setTimeout(r, 3000));
      let w: WorkView;
      try { w = await get<WorkView>(`/api/work/${id}`); } catch { continue; }
      const min = Math.max(0, Math.floor((Date.now() - w.dispatch.started_ms) / 60000));
      if (w.dispatch.state === 'running' || w.dispatch.state === 'answered') {
        line.text = t('chat.bubble.working', { min });
        continue;
      }
      if (w.dispatch.state === 'waiting' && w.question && w.question.seq !== lastQuestion) {
        lastQuestion = w.question.seq;
        const q = w.question;
        say({
          who: 'ai', text: q.question,
          buttons: q.options.map((o, n) => ({
            label: o + (q.recommend === n ? ` · ${t('work.recommended')}` : ''),
            primary: q.recommend === n,
            run: () => answer(id, { option: n }, o),
          })),
        });
        say({ who: 'note', text: t('chat.bubble.answer.own') });
        pendingAnswer = id;
        continue;
      }
      if (w.dispatch.state === 'waiting') continue;
      pendingAnswer = '';
      line.buttons = [{ label: t('chat.bubble.watch'), run: () => go('/work/' + id) }];
      if (w.dispatch.state === 'finished') await finished(id, w.dispatch.detail ?? '', line, from, asked);
      else {
        line.text = t('chat.bubble.ended', { state: t(`work.state.${w.dispatch.state}`), detail: w.dispatch.detail ?? '' });
        if (w.dispatch.state === 'failed') line.buttons = [...(line.buttons ?? []), { label: t('chat.bubble.fix'), primary: true, run: () => retry(asked, w.dispatch.detail ?? '') }];
      }
      return;
    }
  }

  // An answer of your own, typed while the work waits for one.
  let pendingAnswer = $state('');
  async function answer(id: string, body: Record<string, unknown>, shown: string) {
    say({ who: 'me', text: shown });
    try { await post(`/api/work/${id}/answer`, body); pendingAnswer = ''; }
    catch (err: any) { say({ who: 'ai', text: err.message, error: true }); }
  }

  // Ask again, with what went wrong, so the next try mends it.
  function retry(asked: string, why: string) {
    mode = 'change';
    say({ who: 'me', text: t('chat.bubble.fix') });
    run(() => change(`${asked}\n\n${t('chat.bubble.fix.why', { why })}`));
  }

  async function finished(id: string, detail: string, line: Msg, from: string, asked: string) {
    interface V { hash: string; note: string }
    interface U { name: string; versions: V[] }
    let version: V | undefined;
    try {
      const s = await get<{ uis: U[] }>('/api/ui');
      version = s.uis.find((u) => u.name === using.name)?.versions.find((v) => v.note.includes(`(work ${id})`));
    } catch { /* said below */ }
    if (!version) {
      line.text = t('chat.bubble.nobuild', { detail });
      line.buttons = [...(line.buttons ?? []), { label: t('chat.bubble.fix'), primary: true, run: () => retry(asked, detail) }];
      return;
    }
    const hash = version.hash;
    // Shown at once, where you asked: a preview, on this device only, until
    // you say to use it.
    go(previewing(from, hash));
    line.text = t('chat.bubble.ready', { note: version.note.replace(/\s*\(work [^)]*\)$/, '') });
    line.buttons = [
      { label: t('chat.bubble.use'), primary: true, run: () => approveVersion(hash, from) },
      { label: t('chat.bubble.discard'), run: () => { go(from); say({ who: 'note', text: t('chat.bubble.discarded') }); } },
      { label: t('chat.bubble.watch'), run: () => go('/work/' + id) },
    ];
  }

  async function approveVersion(hash: string, from: string) {
    try {
      await post('/api/ui/approve', { ui: using.name, hash });
      say({ who: 'note', text: t('chat.bubble.used') });
      go(from);
      onchanged();
    } catch (err: any) { say({ who: 'ai', text: err.message, error: true }); }
  }

  function keydown(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) { e.preventDefault(); sendOrAnswer(); }
  }
  function sendOrAnswer() {
    const q = text.trim();
    if (pendingAnswer && q && mode === 'change') {
      text = '';
      answer(pendingAnswer, { text: q, public: false }, q);
      return;
    }
    send();
  }
</script>

{#if open}
  <div class="bubble-panel" role="dialog" aria-label={t('chat.bubble.title')}>
    <div class="bubble-head">
      <div class="segments bubble-modes" role="tablist">
        <button type="button" role="tab" class="segment" class:is-on={mode === 'ask'} aria-selected={mode === 'ask'} onclick={() => (mode = 'ask')}>{t('chat.bubble.ask')}</button>
        <button type="button" role="tab" class="segment" class:is-on={mode === 'change'} aria-selected={mode === 'change'} onclick={() => (mode = 'change')}>{t('chat.bubble.change')}</button>
      </div>
      <button type="button" class="bubble-close" onclick={() => (open = false)} aria-label={t('shell.close')}>×</button>
    </div>
    <p class="bubble-where">{t('chat.bubble.where', { page: pageName })} · {using.name === 'default' ? t('ui.default') : using.title}</p>
    <div class="bubble-list" bind:this={list}>
      {#if !msgs.length}
        <p class="note">{mode === 'ask' ? t('chat.bubble.empty.ask') : t('chat.bubble.empty.change')}</p>
      {/if}
      {#each msgs as m, i (i)}
        <div class="bubble-msg {m.who}" class:error={m.error}>
          <p>{m.text}</p>
          {#if m.buttons?.length}
            <div class="chooser">{#each m.buttons as b}<button type="button" class="choose" class:primary={b.primary} disabled={busy} onclick={b.run}>{b.label}</button>{/each}</div>
          {/if}
        </div>
      {/each}
      {#if busy}<div class="bubble-msg ai"><p class="faint">{t('chat.bubble.thinking')}</p></div>{/if}
    </div>
    <form class="bubble-input" onsubmit={(e) => { e.preventDefault(); sendOrAnswer(); }}>
      <textarea bind:this={field} bind:value={text} rows="2" onkeydown={keydown}
        placeholder={pendingAnswer && mode === 'change' ? t('chat.bubble.answer.placeholder') : mode === 'ask' ? t('chat.bubble.placeholder.ask') : t('chat.bubble.placeholder.change')}></textarea>
      <button type="submit" class="choose primary" disabled={busy || !text.trim()}>{t('chat.bubble.send')}</button>
    </form>
  </div>
{/if}
<button type="button" class="bubble-button" class:is-open={open} onclick={() => (open = !open)} aria-label={t('chat.bubble.title')} aria-expanded={open}>
  {#if open}×{:else}<svg viewBox="0 0 24 24" width="24" height="24" aria-hidden="true"><path d="M4 5.5A2.5 2.5 0 0 1 6.5 3h11A2.5 2.5 0 0 1 20 5.5v8a2.5 2.5 0 0 1-2.5 2.5H10l-4.5 4v-4h0A2.5 2.5 0 0 1 4 13.5z" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"/><path d="M8.5 9.5h7M8.5 12.5h4" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/></svg>{/if}
</button>
