<script lang="ts">
  import { post, type Action } from '../lib/api';
  import { kindWord, statusLine } from '../lib/format';
  import { t } from '../lib/i18n';
  import SourceChip from './SourceChip.svelte';
  import { refreshPending } from '../lib/status';

  let { action, ondone }: { action: Action; ondone?: () => void } = $props();
  // The version and nonce this card holds: what approving must present back.
  // The card owns its copy from here on; a fresh list makes fresh cards.
  // svelte-ignore state_referenced_locally
  let current = $state(action);
  // svelte-ignore state_referenced_locally
  let draft = $state(action.draft);
  let busy = $state(false);
  let message = $state('');
  let error = $state('');
  let askingReason = $state(false);
  let reason = $state('');
  let settled = $state<string>(''); // what happened on this card

  async function saveEditIfAny() {
    if (draft !== current.draft) {
      current = await post<Action>(`/api/action/${action.id}/edit`, { payload: draft });
      draft = current.draft;
    }
  }
  async function approve() {
    busy = true; error = '';
    try {
      await saveEditIfAny();
      const done = await post<Action>(`/api/action/${action.id}/approve`, {
        version: current.version, payload_hash: current.payload_hash, nonce: current.nonce,
      });
      current = done; settled = t('action.approved'); refreshPending(); ondone?.();
    } catch (e: any) { error = e.message; busy = false; }
  }
  async function decline() {
    if (!askingReason) { askingReason = true; return; }
    busy = true; error = '';
    try {
      const done = await post<Action>(`/api/action/${action.id}/decline`, { reason });
      current = done; settled = t('action.declined', { reason: done.status_detail }); refreshPending(); ondone?.();
    } catch (e: any) { error = e.message; busy = false; }
  }
  async function withdraw() {
    busy = true; error = '';
    try {
      const done = await post<Action>(`/api/action/${action.id}/decline`, { reason: 'withdrawn before sending' });
      current = done; settled = t('action.withdrawn'); ondone?.();
    } catch (e: any) { error = e.message; busy = false; }
  }
  const editable = $derived(current.status === 'pending' && !settled);
</script>

<!-- Design 06: the one screen where principle five is kept. Evidence before
     the draft; the draft editable in place; the approve button names the
     consequence; a decline wants a reason. -->
<li class="row is-open action {current.status}">
  <div class="meta">
    <span class="who">{kindWord(current.kind)} → {current.target}</span>
    <span class="faint">{statusLine(current)}</span>
  </div>
  {#if current.evidence.length}
    <h4 class="card-label">{t('action.evidence')}</h4>
    <div class="sources">{#each current.evidence as s}<SourceChip src={s} />{/each}</div>
  {/if}
  {#if current.rationale}
    <h4 class="card-label">{t('action.why')}</h4>
    <p class="rationale">{current.rationale}</p>
  {/if}
  <h4 class="card-label">{t('action.draft')}{current.versions > 1 ? ' · ' + t('action.version', { v: current.version }) : ''}</h4>
  <textarea class="draft" bind:value={draft} readonly={!editable}></textarea>
  <p class="note">{t('action.drafted', { where: current.drafted, account: current.account })}</p>
  {#if current.result}
    <h4 class="card-label">{t('action.sent')}</h4>
    <div class="sources"><SourceChip src={current.result} /></div>
  {/if}
  {#if settled}
    <p class="note">{settled}</p>
  {:else if current.status === 'pending'}
    <div class="chooser">
      <button type="button" class="choose primary" disabled={busy} onclick={approve}>{t('action.approve')}</button>
      <button type="button" class="choose lowers" disabled={busy} onclick={decline}>{t('action.decline')}</button>
      {#if askingReason}
        <input class="reason" bind:value={reason} placeholder={t('action.reason')} onkeydown={(e) => { if (e.key === 'Enter') decline(); }} />
      {/if}
      {#if error}<span class="error">{error}</span>{/if}
    </div>
  {:else if current.status === 'approved' && current.can_withdraw}
    <div class="chooser">
      <button type="button" class="choose lowers" disabled={busy} onclick={withdraw}>{t('action.withdraw')}</button>
      {#if error}<span class="error">{error}</span>{/if}
    </div>
  {/if}
</li>
