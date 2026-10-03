<script lang="ts">
  import { get, post, type Quiet, type QuietSender } from '../lib/api';
  import { t } from '../lib/i18n';
  import { go } from '../lib/router';
  import Icon from './Icon.svelte';

  // Design 06, "订阅与广告": the senders of newsletters and promotions, in
  // one place instead of one party each. Open one to see their mail and
  // mark them as junk; or say they are not an advertisement at all.
  let quiet = $state<Quiet | null>(null);
  let error = $state('');
  async function load() {
    try { quiet = await get<Quiet>('/api/quiet'); } catch (e: any) { error = e.message; }
  }
  $effect(() => { load(); });
  async function notAd(s: QuietSender) {
    try {
      await post(`/api/person/${s.id}/category`, { category: 'personal' });
      if (quiet) quiet = { ...quiet, senders: quiet.senders.filter((x) => x.id !== s.id) };
    } catch (e: any) { error = e.message; }
  }
</script>

<div class="conversation">
  <header class="conv-head">
    <button type="button" class="back" onclick={() => go('/chats')} aria-label={t('people.back')}><Icon name="back" size={18} /></button>
    <span class="avatar multi"><Icon name="mail" size={18} /></span>
    <div class="conv-title">
      <span class="conv-name">{t('quiet.title')}</span>
      {#if quiet}<span class="conv-sub"><span class="faint">{t('quiet.summary', { n: quiet.total, s: quiet.senders.length })}</span></span>{/if}
    </div>
  </header>
  <p class="note quiet-note">{t('quiet.note')}</p>
  <div class="stream">
    {#if error}<p class="empty error">{error}</p>{/if}
    {#if !quiet && !error}<p class="empty">{t('loading')}</p>
    {:else if quiet && !quiet.senders.length}<p class="empty">{t('quiet.empty')}</p>{/if}
    <ul class="account-list quiet-senders">
      {#each quiet?.senders ?? [] as s (s.id)}
        <li class="account-row">
          <span class="account-body">
            <button type="button" class="linkish account-name" onclick={() => go(`/chats/${s.id}`)}>{s.name || '—'}</button>
            <span class="account-state">{t(`category.${s.category}`)} · {t('quiet.items', { n: s.items })}{s.last_at ? ' · ' + s.last_at : ''}</span>
          </span>
          <button type="button" class="choose" onclick={() => notAd(s)}>{t('quiet.notad')}</button>
        </li>
      {/each}
    </ul>
  </div>
</div>
