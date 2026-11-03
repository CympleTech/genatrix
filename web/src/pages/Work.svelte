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

  async function stop() {
    if (!id) return;
    try { await post(`/api/work/${id}/stop`); } catch (e: any) { error = e.message; }
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
    <button type="button" class="back" onclick={() => go(detail?.workspace ? '/spaces/' + detail.workspace : '/spaces')}>← {t('work.back')}</button>
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
      <div class="chooser">
        {#if d.state === 'running' || d.state === 'waiting'}<button type="button" class="choose lowers" onclick={stop}>{t('work.stop')}</button>{/if}
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

      <h4 class="card-label">{t('work.said')}</h4>
      <p class="note">{t('work.said.note')}</p>
      <ol class="said">
        {#each events as e (e.seq + ':' + e.n)}
          <li class="said-{e.kind}"><span class="faint">#{e.seq} {e.kind}</span> {e.text}</li>
        {/each}
      </ol>
    {/if}
  {/if}
</section>
