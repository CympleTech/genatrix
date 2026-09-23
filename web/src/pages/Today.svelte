<script lang="ts">
  import { get, type Today } from '../lib/api';
  import { t } from '../lib/i18n';
  import SourceChip from '../components/SourceChip.svelte';
  import ActionCard from '../components/ActionCard.svelte';
  import CommitmentRow from '../components/CommitmentRow.svelte';
  import Empty from '../components/Empty.svelte';

  let today = $state<Today | null>(null);
  let error = $state('');

  async function load() {
    try { today = await get<Today>('/api/today'); error = ''; } catch (e: any) { error = e.message; }
  }
  $effect(() => { load(); });
</script>

<section class="pane">
  {#if error}<Empty text={error} error />
  {:else if !today}<Empty text={t('loading')} />
  {:else}
    {#if !today.digest}
      <p class="headline muted">{t('today.nodigest')}</p>
    {:else}
      <p class="headline">{t('today.digest', { day: today.digest.day, at: today.digest.generated_at, n: today.digest.considered })}</p>
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
    {:else}<Empty text={t('today.nopromises')} />{/if}
  {/if}
</section>
