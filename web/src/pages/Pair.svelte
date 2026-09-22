<script lang="ts">
  import { post, type Device } from '../lib/api';
  import { t } from '../lib/i18n';
  import { route, go } from '../lib/router';
  import { refreshPending } from '../lib/status';

  let code = $state($route.query.get('code') ?? '');
  let name = $state(defaultName());
  let busy = $state(false);
  let error = $state('');
  let done = $state<Device | null>(null);

  function defaultName(): string {
    const ua = navigator.userAgent;
    if (/iPhone/.test(ua)) return 'iPhone';
    if (/iPad/.test(ua)) return 'iPad';
    if (/Android/.test(ua)) return 'Android';
    if (/Macintosh/.test(ua)) return 'Mac';
    return 'a device';
  }
  async function pair(e: Event) {
    e.preventDefault();
    busy = true; error = '';
    try { done = await post<Device>('/api/pair', { code: code.trim(), name }); refreshPending(); }
    catch (err: any) { error = err.message; busy = false; }
  }
</script>

<section class="pane pair">
  <h2 class="headline">{t('pair.title')}</h2>
  {#if done}
    <p class="note">{t('pair.done')}</p>
    <button type="button" class="choose primary" onclick={() => go('/', true)}>{t('pair.open')}</button>
  {:else}
    <p class="note">{t('pair.note')}</p>
    <form onsubmit={pair} class="pair-form">
      <label>{t('pair.name')}<input bind:value={name} autocomplete="off" /></label>
      <label>{t('pair.code')}<input bind:value={code} inputmode="numeric" pattern="[0-9]*" maxlength="6" autocomplete="one-time-code" /></label>
      <button type="submit" class="choose primary" disabled={busy || code.trim().length !== 6}>{t('pair.go')}</button>
      {#if error}<p class="error">{error}</p>{/if}
    </form>
  {/if}
</section>
