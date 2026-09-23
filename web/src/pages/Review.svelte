<script lang="ts">
  import { get, type Review, type ReviewRow } from '../lib/api';
  import { t } from '../lib/i18n';
  import LevelMark from '../components/LevelMark.svelte';
  import LevelChooser from '../components/LevelChooser.svelte';
  import Empty from '../components/Empty.svelte';

  let view = $state<Review | null>(null);
  let items = $state<ReviewRow[]>([]);
  let error = $state('');

  async function load() {
    try { view = await get<Review>('/api/review?count=20'); items = view.items; error = ''; }
    catch (e: any) { error = e.message; }
  }
  async function tally() {
    try { const v = await get<Review>('/api/review?count=0'); if (view) view.tally = v.tally; } catch {}
  }
  function done(id: string) {
    items = items.filter((x) => x.row.id !== id);
    tally();
    if (!items.length) load();
  }
  $effect(() => { load(); });
  const tallyLine = $derived.by(() => {
    const ty = view?.tally;
    if (!ty) return '';
    if (ty.judged === 0) return t('review.none');
    const base = t('review.tally', { judged: ty.judged, reviewed: ty.reviewed });
    if (!ty.reviewed) return base;
    return base + ' · ' + t('review.agreed', { agreed: ty.agreed, reviewed: ty.reviewed, rate: Math.round((100 * ty.agreed) / ty.reviewed) });
  });
</script>

<section class="pane">
  <p class="note">{t('review.note')}</p>
  <p class="headline small">{tallyLine}</p>
  {#if error}<Empty text={error} error />
  {:else if !view}<Empty text={t('loading')} />
  {:else if !items.length}<Empty text={view.tally.judged === 0 ? t('review.empty') : t('review.alldone')} />
  {:else}
    <ol class="rows panel">
      {#each items as entry (entry.row.id)}
        <li class="row is-open">
          <div class="meta">
            <time>{entry.row.at}</time>
            <LevelMark level={entry.row.level} reason={entry.row.level_reason} />
            <span class="who">{entry.row.author}</span>
          </div>
          {#if entry.subject}<p class="preview static">{entry.subject}</p>{/if}
          <p class="excerpt">{entry.text}</p>
          <p class="note">{t('review.model', { level: entry.model_level })}</p>
          <LevelChooser itemId={entry.row.id} current={entry.row.level} agreeWith={entry.model_level} ondone={() => done(entry.row.id)} />
        </li>
      {/each}
    </ol>
    <button type="button" class="choose" onclick={load}>{t('review.more')}</button>
  {/if}
</section>
