<script lang="ts">
  import { post } from '../lib/api';
  import { t } from '../lib/i18n';

  // Design 06 v0.21: give a GitHub token, for one space or as the default.
  // The core asks GitHub whose it is and keeps it in the keychain; the page
  // never sees it again.
  let { workspace = null, onsaved, oncancel }: {
    workspace?: string | null; onsaved: (login: string) => void; oncancel?: () => void;
  } = $props();

  let token = $state('');
  let busy = $state(false);
  let error = $state('');

  async function save(e: Event) {
    e.preventDefault();
    busy = true; error = '';
    try {
      const r = await post<{ login: string }>('/api/github/token', { workspace, token: token.trim() });
      token = '';
      onsaved(r.login);
    } catch (err: any) { error = err.message; }
    busy = false;
  }
</script>

<form class="setup-form gh-token" onsubmit={save}>
  <label>{t('github.token')}
    <input type="password" bind:value={token} autocomplete="off" autocapitalize="off" spellcheck="false" placeholder="github_pat_…" required />
    <span class="field-note">{t('github.token.note')} <a href="https://github.com/settings/personal-access-tokens/new" target="_blank" rel="noopener noreferrer">{t('github.token.make')}</a></span>
  </label>
  {#if error}<p class="note error">{error}</p>{/if}
  <div class="chooser">
    <button type="submit" class="choose primary" disabled={busy || !token.trim()}>{busy ? t('github.checking') : t('github.save')}</button>
    {#if oncancel}<button type="button" class="choose lowers" onclick={oncancel}>{t('accounts.cancel')}</button>{/if}
  </div>
</form>
