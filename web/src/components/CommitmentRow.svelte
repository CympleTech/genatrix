<script lang="ts">
  import { post, type Commitment } from '../lib/api';
  import { t } from '../lib/i18n';
  import SourceChip from './SourceChip.svelte';

  let { c, ondone }: { c: Commitment; ondone?: () => void } = $props();
  let busy = $state(false);
  let error = $state('');
  let gone = $state(false);

  async function act(standing: string, status: string | null) {
    busy = true;
    try { await post(`/api/commitment/${c.id}`, { standing, status }); gone = true; ondone?.(); }
    catch (e: any) { error = e.message; busy = false; }
  }
</script>

{#if !gone}
<li class="row is-open commitment {c.status}">
  <div class="meta">
    <span class="who">{c.mine ? t('commitment.you') : c.from}</span>
    {#if c.to}<span>{c.mine ? '→ ' + c.to : t('commitment.toyou')}</span>{/if}
    <span class="thread">{c.due ? t('commitment.by', { due: c.due }) : t('commitment.nodate')}</span>
    <span class="standing">{c.standing === 'confirmed' ? t('commitment.confirmed') : t('commitment.inferred')}</span>
    {#if c.status === 'overdue'}<span class="error">{t('commitment.overdue')}</span>{/if}
  </div>
  <p class="preview static">{c.what}</p>
  <div class="sources">{#each c.evidence as s}<SourceChip src={s} />{/each}</div>
  <div class="chooser">
    {#if c.standing !== 'confirmed'}
      <button type="button" class="choose" disabled={busy} onclick={() => act('confirmed', null)}>{t('commitment.yes')}</button>
    {/if}
    <button type="button" class="choose" disabled={busy} onclick={() => act('confirmed', 'done')}>{t('commitment.done')}</button>
    <button type="button" class="choose lowers" disabled={busy} onclick={() => act('rejected', 'cancelled')}>{t('commitment.not')}</button>
    {#if error}<span class="error">{error}</span>{/if}
  </div>
</li>
{/if}
