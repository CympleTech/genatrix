<script lang="ts">
  import { get, type Today } from '../lib/api';
  import { t } from '../lib/i18n';
  import SourceChip from '../components/SourceChip.svelte';
  import ActionCard from '../components/ActionCard.svelte';
  import CommitmentRow from '../components/CommitmentRow.svelte';
  import Empty from '../components/Empty.svelte';

  let today = $state<Today | null>(null);
  let error = $state('');
  const needsReply = $derived(
    today?.digest?.groups.find((g) => g.group === 'needs_reply')?.points.length ?? 0,
  );
  // Always the same four, zeros included, so the card keeps its shape from
  // one morning to the next.
  const stats = $derived.by(() => {
    const count = (g: string) => today?.digest?.groups.find((x) => x.group === g)?.points.length ?? 0;
    return [
      { key: 'needs_reply', n: count('needs_reply') },
      // The day's promises, from the same digest as the other figures; the
      // whole backlog is below the digest and on each person's page.
      { key: 'promised', n: count('promised') },
      { key: 'worth_knowing', n: count('worth_knowing') },
      { key: 'waiting', n: today?.pending_actions ?? 0 },
    ];
  });

  async function load() {
    try { today = await get<Today>('/api/today'); error = ''; } catch (e: any) { error = e.message; }
  }
  $effect(() => { load(); });
</script>

<section class="pane">
  {#if error}<Empty text={error} error />
  {:else if !today}<Empty text={t('loading')} />
  {:else}
    <div class="hero">
      {#if !today.digest}
        <p class="hero-line">{t('today.nodigest')}</p>
        <p class="hero-foot">{t('today.nodigest.foot')}</p>
      {:else}
        <p class="hero-eyebrow">{t('today.hero.day', { day: today.digest.day })}</p>
        <p class="hero-line">
          {needsReply === 0 ? t('today.hero.needsreply.zero') : needsReply === 1 ? t('today.hero.needsreply.one') : t('today.hero.needsreply', { n: needsReply })}
        </p>
        <p class="hero-foot">{t('today.hero.made', { at: today.digest.generated_at, n: today.digest.considered })}</p>
        <div class="hero-stats">
          {#each stats as st}
            <div class="hero-stat" class:zero={st.n === 0}>
              <span class="hero-stat-n">{st.n}</span>
              <span class="hero-stat-label">{t(`today.stat.${st.key}`)}</span>
            </div>
          {/each}
        </div>
      {/if}
    </div>
    {#if today.digest}
      {#each today.digest.groups.filter((g) => g.group !== 'promised') as g}
        <div class="card digest-card">
        <h2 class="group-title">{t(`group.${g.group}`)} ({g.points.length})</h2>
        <ol class="points">
          {#if !g.points.length}<li class="point empty-point">{t('point.nothing')}</li>{/if}
          {#each g.points as p}
            <li class="point" class:unfounded={!p.sources.length}>
              <span class="text">{p.text}</span>
              {#if p.sources.length}
                <div class="sources">{#each p.sources as s}<SourceChip src={s} />{/each}</div>
              {:else}<span class="note">{t('point.nosource')}</span>{/if}
            </li>
          {/each}
        </ol>
        </div>
      {/each}
    {/if}

    <h2 class="group-title">{t('today.waiting')}</h2>
    {#if today.actions.length}
      <ol class="rows cards">{#each today.actions as a (a.id)}<ActionCard action={a} ondone={load} />{/each}</ol>
    {:else}<Empty text={t('today.nowaiting')} />{/if}

    <h2 class="group-title">{t('today.promised')}</h2>
    {#if today.commitments.length}
      <ol class="rows cards">{#each today.commitments as c (c.id)}<CommitmentRow {c} />{/each}</ol>
      {#if today.commitments_total > today.commitments.length}
        <p class="note mt-3">{t('today.morepromises', { n: today.commitments_total - today.commitments.length })}</p>
      {/if}
    {:else}<Empty text={t('today.nopromises')} />{/if}
  {/if}
</section>
