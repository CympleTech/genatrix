<script lang="ts">
  import { get, post, type Action, type WorkDetail, type WorkEvent } from '../lib/api';
  import { t } from '../lib/i18n';
  import { go } from '../lib/router';
  import ActionCard from '../components/ActionCard.svelte';
  import Empty from '../components/Empty.svelte';

  // Design 13, "界面": one piece of work handed to executors: watch it,
  // stop it, answer it, read what it changed. What the executors said is
  // shown as they said it, as material, never as advice from Genatrix.
  // Work is begun in a space (design 06 v0.14); this page is where it is
  // followed.
  let { id }: { id: string | null } = $props();

  let error = $state('');
  // There is no list here any more: work is begun and kept in a space
  // (design 06 v0.14). An old link to /work goes there.
  $effect(() => { if (!id) go('/spaces', true); });

  // One dispatch: its record, followed while it runs.
  let detail = $state<WorkDetail | null>(null);
  let events = $state<WorkEvent[]>([]);
  let diff = $state<string | null>(null);
  let alive = true;

  // Executors stream a few words at a time, and each piece is kept as it
  // came. Shown that way it is unreadable, so the page joins the pieces:
  // while a step runs, one line says what it is doing; what it concludes
  // is shown whole; the rest is folded away. The record is not changed.
  interface Block { seq: number; kind: string; text: string; failed?: boolean }
  const story = $derived.by(() => {
    const blocks: Block[] = [];
    const plans = new Map<number, string>();
    const tools = new Map<number, number>();
    for (const e of events) {
      if (e.kind === 'plan') { plans.set(e.seq, e.text); continue; }
      if (e.kind === 'tool_update') {
        // Only a failure says anything the tool line did not.
        if (/:\s*failed$/.test(e.text)) blocks.push({ seq: e.seq, kind: 'tool', text: e.text, failed: true });
        continue;
      }
      if (e.kind === 'tool') tools.set(e.seq, (tools.get(e.seq) ?? 0) + 1);
      const last = blocks[blocks.length - 1];
      if (last && last.seq === e.seq && last.kind === e.kind && (e.kind === 'thought' || e.kind === 'message')) last.text += e.text;
      else blocks.push({ seq: e.seq, kind: e.kind, text: e.text });
    }
    const said = blocks.filter((b) => b.kind === 'message' && b.text.trim());
    return { blocks, plans, tools, said };
  });
  const stepOf = (seq: number) => detail?.steps.find((s) => s.seq === seq) ?? null;
  const stepLabel = (seq: number) => {
    const s = stepOf(seq);
    if (!s) return `#${seq}`;
    const k = `work.step.${s.step}`; const w = t(k);
    return `${w === k ? s.step : w} · ${s.executor}${s.round > 1 ? ' · ' + t('work.round', { n: s.round }) : ''}`;
  };
  // What the step running now is doing, from the last thing it sent.
  const now = $derived.by(() => {
    const last = events[events.length - 1];
    if (!last) return null;
    const seq = last.seq;
    const recent = [...events].reverse().find((e) => e.seq === seq && e.kind !== 'tool_update' && e.kind !== 'plan');
    const doing = !recent ? t('work.now.starting')
      : recent.kind === 'thought' ? t('work.now.thinking')
      : recent.kind === 'message' ? t('work.now.writing')
      : recent.kind === 'tool' ? t('work.now.tool', { tool: recent.text.slice(0, 80) })
      : t('work.now.working');
    return { seq, doing, tools: story.tools.get(seq) ?? 0, plan: story.plans.get(seq) ?? '' };
  });
  let showAll = $state(false);

  // The object an executor ends its turn with (design 13, "每一轮的交代"),
  // read out of its words so the page shows the title and summary, not the
  // JSON. What came before it is shown as it is.
  interface Ending { prose: string; title?: string | null; summary?: string; asked?: boolean }
  function ending(text: string): Ending {
    const at = [...text.matchAll(/\{\s*"genatrix"\s*:/g)].pop()?.index;
    if (at === undefined) return { prose: text.trim() };
    let tail = text.slice(at).replace(/\s*```\s*$/, '').trim();
    try {
      const r = JSON.parse(tail);
      const prose = text.slice(0, at).replace(/```(json)?\s*$/, '').trim();
      if (r.genatrix === 'done') return { prose, title: r.title ?? null, summary: String(r.summary ?? '') };
      if (r.genatrix === 'ask') return { prose, asked: true };
    } catch { /* not the object after all */ }
    return { prose: text.trim() };
  }
  $effect(() => () => { alive = false; });
  async function follow(which: string) {
    events = []; detail = null; diff = null;
    let after = { seq: 0, n: 0 };
    while (alive && id === which) {
      try {
        const d = await get<WorkDetail>(`/api/work/${which}?seq=${after.seq}&n=${after.n}`);
        if (id !== which) return;
        detail = d;
        if (d.events.length) {
          events = [...events, ...d.events];
          const last = d.events[d.events.length - 1];
          after = { seq: last.seq, n: last.n };
        }
        // Answered work is picked up by the core within a few seconds.
        if (d.dispatch.state !== 'running' && d.dispatch.state !== 'answered') return;
      } catch (e: any) { error = e.message; return; }
      await new Promise((r) => setTimeout(r, 2000));
    }
  }
  $effect(() => { if (id) follow(id); });

  // Stopping cancels the executor's turn; it gets a few seconds to wind
  // down, so the page says it is stopping until the record says stopped.
  let stopping = $state(false);
  $effect(() => { id; stopping = false; });
  async function stop() {
    if (!id) return;
    stopping = true; error = '';
    try { await post(`/api/work/${id}/stop`); } catch (e: any) { error = e.message; stopping = false; }
  }
  // Design 13, "等你回答": one of its options, or your own words. Your own
  // words count as personal unless you say they are not.
  let reply = $state('');
  let replyPublic = $state(false);
  let replying = $state(false);
  let replyError = $state('');
  async function answer(option: number | null) {
    if (!id) return;
    replying = true; replyError = '';
    try {
      await post(`/api/work/${id}/answer`, option === null ? { text: reply, public: replyPublic } : { option });
      reply = ''; replyPublic = false;
      follow(id);
    } catch (e: any) { replyError = e.message; }
    replying = false;
  }

  // A push GitHub refused (or you declined) is offered again as a new card.
  // Its failure shows beside the button; `error` would replace the page.
  let offered = $state<Action | null>(null);
  let offerError = $state('');
  async function pushAgain() {
    if (!id) return;
    offerError = '';
    try {
      offered = await post<Action>(`/api/work/${id}/push-again`);
      if (detail) detail = { ...detail, push_again: false };
    } catch (e: any) { offerError = e.message; }
  }
  async function toggleDiff() {
    if (diff !== null) { diff = null; return; }
    try { diff = (await get<{ diff: string }>(`/api/work/${id}/diff`)).diff; } catch (e: any) { error = e.message; }
  }
  const when = (ms: number) => new Date(ms).toLocaleString();
  const stateWord = (s: string) => t(`work.state.${s}`);
</script>

<section class="pane work">
  {#if id}
    <button type="button" class="back" onclick={() => go(detail?.workspace ? '/spaces/' + detail.workspace + '/work' : '/spaces')}>← {t('work.back')}</button>
    {#if error}<Empty text={error} error />
    {:else if !detail}<Empty text={t('loading')} />
    {:else}
      {@const d = detail.dispatch}
      <h2 class="group-title">{d.project} · {d.recipe}</h2>
      <p class="note">
        <span class="state {d.state}">{stateWord(d.state)}</span> · {d.branch} · {when(d.started_ms)}
        {#if d.review_passed === true} · {t('work.review.pass')}{:else if d.review_passed === false} · {t('work.review.changes')}{/if}
      </p>
      {#if d.detail}<p class="note">{d.detail}</p>{/if}
      {#if stopping && (d.state === 'running' || d.state === 'answered')}<p class="note">{t('work.stopping.note')}</p>{/if}
      <div class="chooser">
        {#if d.state === 'running' || d.state === 'waiting' || d.state === 'answered'}<button type="button" class="choose danger" disabled={stopping} onclick={stop}>{stopping ? t('work.stopping') : t('work.stop')}</button>{/if}
        {#if d.tip}<button type="button" class="choose" onclick={toggleDiff}>{diff === null ? t('work.diff') : t('work.diff.hide')}</button>{/if}
        {#if detail.push_again}<button type="button" class="choose" onclick={pushAgain}>{t('work.push_again')}</button>{/if}
      </div>
      {#if detail.question && d.state === 'waiting'}
        {@const q = detail.question}
        <div class="asked">
          <h4 class="card-label">{t('work.asked')}</h4>
          {#if q.progress}<p class="note">{q.progress}</p>{/if}
          <p class="asked-question">{q.question}</p>
          {#if q.options.length}
            <div class="chooser">
              {#each q.options as o, i}
                <button type="button" class="choose" class:primary={q.recommend === i} disabled={replying} onclick={() => answer(i)}>
                  {o}{q.recommend === i ? ' · ' + t('work.recommended') : ''}
                </button>
              {/each}
            </div>
          {/if}
          <textarea class="draft" bind:value={reply} placeholder={t('work.reply.placeholder')}></textarea>
          <label class="quiet-toggle"><input type="checkbox" bind:checked={replyPublic} /> {t('work.public')}</label>
          <div class="chooser">
            <button type="button" class="choose" disabled={replying || !reply.trim()} onclick={() => answer(null)}>{t('work.reply.send')}</button>
          </div>
          <p class="field-note">{t('work.reply.note')}</p>
          <div aria-live="polite">{#if replyError}<p class="note error">{replyError}</p>{/if}</div>
        </div>
      {/if}
      {#if detail.offers?.length}
        {@const failed = detail.offers.find((o) => o.status === 'failed')}
        {@const pending = detail.offers.some((o) => o.status === 'pending')}
        {@const merged = detail.offers.every((o) => o.status === 'executed' || o.status === 'declined')}
        <!-- Finished here is not on GitHub yet: say so, and show the push
             and the pull request, whatever became of them, where the work
             is read. -->
        {#if !merged}
          <div class="next" class:bad={!!failed}>
            <h4 class="card-label">{failed ? t('work.next.failed') : pending ? t('work.next') : t('work.next.going')}</h4>
            {#if failed}<p class="note">{t('work.next.failed.note')}</p>
            {:else if pending}<p class="note">{t('work.next.note', { c: d.tip ? d.tip.slice(0, 7) : '' })}</p>{/if}
          </div>
        {/if}
        <ol class="rows cards">
          {#each detail.offers as w (w.id)}<ActionCard action={w} />{/each}
        </ol>
      {/if}
      {#if offerError}<p class="note error">{offerError}</p>{/if}
      {#if offered}<ol class="rows cards"><ActionCard action={offered} /></ol>{/if}
      {#if diff !== null}<pre class="diff">{diff}</pre>{/if}

      <h4 class="card-label">{t('work.steps')}</h4>
      <ol class="rows">
        {#each detail.steps as s (s.seq)}
          <li class="row">
            <div class="meta">
              <span class="who">{s.step}{s.round > 1 ? ' · ' + t('work.round', { n: s.round }) : ''}</span>
              <span class="faint">{s.executor} · {s.role} · {s.state}</span>
            </div>
            {#if s.verdict}
              <p class="text">{s.verdict.pass ? t('work.review.pass') : t('work.review.changes')}{s.verdict.notes ? ': ' + s.verdict.notes : ''}</p>
            {/if}
            {#if s.commit_id}<p class="faint">{t('work.commit', { c: s.commit_id.slice(0, 10) })}</p>{/if}
            {#if s.error}<p class="error">{s.error}</p>{/if}
          </li>
        {/each}
      </ol>

      {#if (d.state === 'running' || d.state === 'answered') && now}
        <div class="now" aria-live="polite">
          <p class="now-line"><span class="now-dot"></span><span><b>{stepLabel(now.seq)}</b>：{now.doing}{#if now.tools} <span class="faint">· {t('work.now.tools', { n: now.tools })}</span>{/if}</span></p>
          {#if now.plan}
            <ul class="now-plan">
              {#each now.plan.split('\n') as line}
                {@const m = line.match(/^\[(\w+)\]\s*(.*)$/)}
                <li class={m?.[1] ?? ''}>{m ? m[2] : line}</li>
              {/each}
            </ul>
          {/if}
        </div>
      {/if}

      <h4 class="card-label">{t('work.said')}</h4>
      <p class="note">{t('work.said.note')}</p>
      {#if !story.said.length}
        <p class="note">{d.state === 'running' || d.state === 'answered' ? t('work.said.waiting') : t('work.said.none')}</p>
      {:else}
        {#each story.said as b, i (i)}
          {@const end = ending(b.text)}
          <div class="said-block">
            <p class="said-who">{stepLabel(b.seq)}</p>
            {#if end.title || end.summary}
              <div class="said-text said-summary">
                {#if end.title}<p class="said-title">{end.title}</p>{/if}
                {#if end.summary}<p>{end.summary}</p>{/if}
              </div>
            {/if}
            {#if end.prose}<p class="said-text">{end.prose}</p>{/if}
            {#if end.asked}<p class="faint">{t('work.said.asked')}</p>{/if}
          </div>
        {/each}
      {/if}
      {#if story.blocks.length}
        <button type="button" class="linkish" onclick={() => (showAll = !showAll)}>{showAll ? t('work.process.hide') : t('work.process.show', { n: story.blocks.length })}</button>
        {#if showAll}
          <ol class="said">
            {#each story.blocks as b, i (i)}
              <li class="said-{b.kind}" class:failed={b.failed}><span class="faint">{stepLabel(b.seq)} · {t('work.kind.' + b.kind) === 'work.kind.' + b.kind ? b.kind : t('work.kind.' + b.kind)}</span> {b.text}</li>
            {/each}
          </ol>
        {/if}
      {/if}
    {/if}
  {/if}
</section>
