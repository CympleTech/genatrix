<script lang="ts">
  import { get, type Ledger } from '../lib/api';
  import { t } from '../lib/i18n';
  import { kindWord } from '../lib/format';
  import LevelMark from '../components/LevelMark.svelte';
  import RunRecord from '../components/RunRecord.svelte';
  import Empty from '../components/Empty.svelte';

  let view = $state<Ledger | null>(null);
  let error = $state('');
  let openRun = $state<string | null>(null);

  $effect(() => {
    get<Ledger>('/api/ledger').then((v) => { view = v; }).catch((e) => { error = e.message; });
  });
</script>

<!-- Design 06, "记录": the three books together. The first line is a
     sentence and a number; nothing here is a chart. -->
<section class="pane">
  {#if error}<Empty text={error} error />
  {:else if !view}<Empty text={t('loading')} />
  {:else}
    <p class="headline">{view.headline}</p>
    <p class="note" class:error={!view.verified}>{view.verified ? t('records.chain', { n: view.entries }) : t('records.broken', { n: view.entries })}</p>

    <h2 class="group-title">{t('records.left')}</h2>
    {#if !view.calls.length}<Empty text={t('records.nocalls')} />{/if}
    <ol class="rows compact">
      {#each view.calls as call}
        <li class="row">
          <div class="meta">
            <time>{call.at}</time>
            <LevelMark level={call.level} />
            <span class="who">{call.purpose}</span>
            <span>{call.location === 'cloud' ? `${call.target} · cloud` : call.target}</span>
            <span class="thread">{call.items} item{call.items === 1 ? '' : 's'}</span>
          </div>
          {#if call.payload}<pre class="payload">{call.payload}</pre>
          {:else}<p class="preview static muted">{t('records.local')}</p>{/if}
        </li>
      {/each}
    </ol>

    <h2 class="group-title">{t('records.actions')}</h2>
    {#if !view.actions.length}<Empty text={t('records.noactions')} />{/if}
    <ol class="rows compact">
      {#each view.actions as ev}
        <li class="row">
          <div class="meta">
            <time>{ev.at}</time>
            <span class="who">{ev.event} · {ev.by}</span>
            <span>{kindWord(ev.kind)} · v{ev.version}</span>
            <span class="thread">{ev.action}</span>
          </div>
          {#if ev.detail}<p class="preview static">{ev.detail}</p>{/if}
        </li>
      {/each}
    </ol>

    <h2 class="group-title">{t('records.runs')}</h2>
    <ol class="rows compact">
      {#each view.runs as r (r.id)}
        <li class="row">
          <button type="button" class="row-head" onclick={() => (openRun = openRun === r.id ? null : r.id)}>
            <div class="meta">
              <time>{r.at}</time>
              <span class="who">{r.task}</span>
              <span>{r.end === 'running' ? t('records.running') : t('records.steps', { end: r.end, steps: r.steps, max: r.max_steps })}</span>
            </div>
            {#if r.reason}<p class="preview static">{r.reason}</p>{/if}
          </button>
          {#if openRun === r.id}<RunRecord runId={r.id} />{/if}
        </li>
      {/each}
    </ol>
  {/if}
</section>
