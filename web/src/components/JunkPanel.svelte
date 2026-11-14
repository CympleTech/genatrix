<script lang="ts">
  import { get, post } from '../lib/api';
  import { t } from '../lib/i18n';

  // Design 01 and 06, "垃圾": what the user marked as junk. Taking the mark
  // away lets new messages in again; what was removed stays removed.
  interface Junk { kind: 'person' | 'thread'; id: string; name: string; at: string }
  let junk = $state<Junk[] | null>(null);
  let error = $state('');
  async function load() {
    try { junk = await get<Junk[]>('/api/junk'); } catch (e: any) { error = e.message; }
  }
  $effect(() => { load(); });
  // Off the list for good (design 01 v0.7): asked once, on the row.
  let asking = $state('');
  async function forget(j: Junk) {
    try { await post(`/api/junk/${j.kind}/${j.id}/forget`, {}); asking = ''; await load(); }
    catch (e: any) { error = e.message; }
  }
  async function restore(j: Junk) {
    try { await post(`/api/junk/${j.kind}/${j.id}/restore`, {}); await load(); }
    catch (e: any) { error = e.message; }
  }
</script>

<div class="section">
  <h2 class="group-title">{t('junk.title')}</h2>
  <p class="note">{t('junk.note')}</p>
  {#if error}<p class="note error">{error}</p>{/if}
  {#if junk && !junk.length}<p class="note">{t('junk.none')}</p>{/if}
  <ul class="account-list">
    {#each junk ?? [] as j (j.kind + j.id)}
      <li class="account-row">
        <span class="account-body">
          <span class="account-name">{j.name || t(j.kind === 'person' ? 'junk.someone' : 'junk.agroup')}</span>
          <span class="account-state">{t(j.kind === 'person' ? 'junk.kind.person' : 'junk.kind.thread')} · {j.at.slice(0, 10)}</span>
        </span>
        {#if asking === j.kind + j.id}
          <span class="junk-ask">
            <span class="note">{t('junk.forget.confirm')}</span>
            <span class="chooser">
              <button type="button" class="choose danger" onclick={() => forget(j)}>{t('junk.forget.yes')}</button>
              <button type="button" class="choose lowers" onclick={() => (asking = '')}>{t('agents.cancel')}</button>
            </span>
          </span>
        {:else}
          <span class="chooser">
            <button type="button" class="choose" onclick={() => restore(j)}>{t('junk.restore')}</button>
            <button type="button" class="choose danger" onclick={() => (asking = j.kind + j.id)}>{t('junk.forget')}</button>
          </span>
        {/if}
      </li>
    {/each}
  </ul>
</div>
