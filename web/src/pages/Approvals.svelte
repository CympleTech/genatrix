<script lang="ts">
  import { get, type Action } from '../lib/api';
  import { t } from '../lib/i18n';
  import ActionCard from '../components/ActionCard.svelte';
  import Empty from '../components/Empty.svelte';

  let pending = $state<Action[] | null>(null);
  let history = $state<Action[]>([]);
  let error = $state('');

  async function load() {
    try {
      pending = await get<Action[]>('/api/actions?status=pending&limit=100');
      const all = await get<Action[]>('/api/actions?limit=60');
      history = all.filter((a) => a.status !== 'pending');
      error = '';
    } catch (e: any) { error = e.message; }
  }
  $effect(() => { load(); });
</script>

<section class="pane">
  <p class="note">{t('approvals.note')}</p>
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
