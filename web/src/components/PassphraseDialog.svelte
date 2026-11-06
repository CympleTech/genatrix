<script lang="ts">
  import { passphraseAsk } from '../lib/api';
  import { t } from '../lib/i18n';

  // Design 06 v0.7: the admin passphrase is typed for the one request that
  // needs it. It lives in this field until the request is sent, and nowhere
  // after: not in the browser's storage, not in the page's state.
  let value = $state('');
  let field = $state<HTMLInputElement | null>(null);

  $effect(() => {
    if ($passphraseAsk) { value = ''; queueMicrotask(() => field?.focus()); }
  });
  function give() {
    const ask = $passphraseAsk;
    const p = value;
    value = '';
    ask?.answer(p || null);
  }
  function cancel() {
    const ask = $passphraseAsk;
    value = '';
    ask?.answer(null);
  }
</script>

{#if $passphraseAsk}
  <div class="passphrase-scrim" onclick={cancel} role="presentation"></div>
  <div class="passphrase-sheet" role="dialog" aria-modal="true" aria-labelledby="passphrase-title">
  <form class="grid gap-3" onsubmit={(e) => { e.preventDefault(); give(); }}>
    <h2 id="passphrase-title" class="group-title">{t('admin.ask.title')}</h2>
    <p class="note">{t('admin.ask.note')}</p>
    {#if $passphraseAsk.why}<p class="note error">{$passphraseAsk.why}</p>{/if}
    <input bind:this={field} type="password" autocomplete="off" bind:value aria-label={t('admin.passphrase')} />
    <div class="chooser">
      <button type="submit" class="choose primary" disabled={!value}>{t('admin.ask.go')}</button>
      <button type="button" class="choose lowers" onclick={cancel}>{t('accounts.cancel')}</button>
    </div>
  </form>
  </div>
{/if}
