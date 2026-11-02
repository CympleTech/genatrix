<script lang="ts">
  import { get, type Action, type WorkOverview } from '../lib/api';
  import { go } from '../lib/router';
  import { t } from '../lib/i18n';
  import ActionCard from '../components/ActionCard.svelte';
  import Empty from '../components/Empty.svelte';

  let pending = $state<Action[] | null>(null);
  let history = $state<Action[]>([]);
  let error = $state('');
  // Work that stopped to ask you something (design 13, "等你回答").
  let waiting = $state(0);

  async function load() {
    try {
      pending = await get<Action[]>('/api/actions?status=pending&limit=100');
      const all = await get<Action[]>('/api/actions?limit=60');
      history = all.filter((a) => a.status !== 'pending');
      try {
        const work = await get<WorkOverview>('/api/work');
        waiting = work.dispatches.filter((d) => d.state === 'waiting').length;
      } catch { waiting = 0; }
      error = '';
    } catch (e: any) { error = e.message; }
  }
  $effect(() => { load(); });
</script>

<section class="pane">
  <p class="note">{t('approvals.note')}</p>
  {#if waiting}
    <p class="note"><button type="button" class="linkish" onclick={() => go('/work')}>{t('approvals.waiting', { n: waiting })} →</button></p>
  {/if}
  {#if error}<Empty text={error} error />
  {:else if !pending}<Empty text={t('loading')} />
  {:else if !pending.length}<Empty text={t('approvals.empty')} />
  {:else}
    <ol class="rows cards">{#each pending as a (a.id)}<ActionCard action={a} ondone={load} />{/each}</ol>
  {/if}
  {#if history.length}
    <h2 class="group-title">{t('approvals.decided')}</h2>
    <ol class="rows cards">{#each history as a (a.id)}<ActionCard action={a} />{/each}</ol>
  {/if}
</section>
