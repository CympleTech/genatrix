<script lang="ts">
  import { get } from '../lib/api';
  import { t } from '../lib/i18n';
  import { go } from '../lib/router';
  import AccountsPanel from '../components/AccountsPanel.svelte';
  import ModelsPanel from '../components/ModelsPanel.svelte';
  import Icon from '../components/Icon.svelte';

  // Design 09, "首次运行" and "向导的形态"; design 10, "第一次外部用户": the
  // three things a person is told before anything starts, then the machine,
  // the models, the first account, and who you are.
  interface Check { what: string; ok: boolean; required: boolean; found: string }
  interface SetupView { local: boolean; accounts: { kind: string; name: string }[] }

  const steps = ['welcome', 'check', 'models', 'account', 'done'] as const;
  let step = $state<(typeof steps)[number]>('welcome');
  let checks = $state<Check[] | null>(null);
  let setup = $state<SetupView | null>(null);

  $effect(() => {
    if (step === 'check' && !checks) get<Check[]>('/api/system/check').then((c) => { checks = c; });
  });
  // Know when the first account is there, to let the wizard move on.
  $effect(() => {
    if (step !== 'account' && step !== 'done') return;
    const load = () => get<SetupView>('/api/setup').then((v) => { setup = v; }).catch(() => {});
    load();
    const timer = setInterval(load, 3000);
    return () => clearInterval(timer);
  });

  const blocked = $derived(checks?.some((c) => c.required && !c.ok) ?? true);
  const lowMemory = $derived(checks?.some((c) => c.what === 'memory' && !c.ok) ?? false);
  const hasAccount = $derived((setup?.accounts.length ?? 0) > 0);
  const index = $derived(steps.indexOf(step));

  function skip() {
    try { localStorage.setItem('genatrix.welcome.skipped', '1'); } catch {}
    go('/', true);
  }
  function finish() {
    try { localStorage.setItem('genatrix.welcome.skipped', '1'); } catch {}
    go('/', true);
  }
</script>

<section class="pane welcome">
  <div class="welcome-top">
    <ol class="welcome-steps">
      {#each steps as s, i}<li class:on={i === index} class:done={i < index}></li>{/each}
    </ol>
    {#if step !== 'done'}<button type="button" class="back" onclick={skip}>{t('welcome.skip')}</button>{/if}
  </div>

  {#if step === 'welcome'}
    <div class="welcome-card">
      <span class="welcome-mark"><Icon name="today" size={28} /></span>
      <h2 class="headline">{t('welcome.title')}</h2>
      <p class="welcome-lede">{t('welcome.lede')}</p>
      <ul class="consent">
        <li><b>{t('welcome.consent.1.title')}</b> {t('welcome.consent.1')}</li>
        <li><b>{t('welcome.consent.2.title')}</b> {t('welcome.consent.2')}</li>
        <li><b>{t('welcome.consent.3.title')}</b> {t('welcome.consent.3')}</li>
      </ul>
      <button type="button" class="choose primary" onclick={() => (step = 'check')}>{t('welcome.understood')}</button>
    </div>
  {:else if step === 'check'}
    <div class="welcome-card">
      <h2 class="headline">{t('welcome.check.title')}</h2>
      {#if !checks}<p class="note">{t('loading')}</p>
      {:else}
        <ul class="checks">
          {#each checks as c}
            <li class:ok={c.ok} class:bad={!c.ok && c.required} class:warn={!c.ok && !c.required}>
              <span class="check-mark">{c.ok ? '✓' : c.required ? '✕' : '!'}</span>
              <span class="check-body">
                <span class="check-name">{t(`welcome.check.${c.what}`)}</span>
                <span class="faint">{c.found}</span>
              </span>
            </li>
          {/each}
        </ul>
        {#if blocked}<p class="note error">{t('welcome.check.blocked')}</p>{/if}
        {#if lowMemory}<p class="note">{t('welcome.check.lowMemory')}</p>{/if}
        <button type="button" class="choose primary" disabled={blocked} onclick={() => (step = 'models')}>{t('welcome.next')}</button>
      {/if}
    </div>
  {:else if step === 'models'}
    <p class="note">{t('welcome.models.note')}</p>
    <ModelsPanel />
    <button type="button" class="choose primary" onclick={() => (step = 'account')}>{t('welcome.next')}</button>
  {:else if step === 'account'}
    <p class="note">{t('welcome.account.note')}</p>
    <AccountsPanel />
    <button type="button" class="choose primary" disabled={!hasAccount} onclick={() => (step = 'done')}>{t('welcome.next')}</button>
  {:else}
    <div class="welcome-card">
      <span class="welcome-mark"><Icon name="approvals" size={28} /></span>
      <h2 class="headline">{t('welcome.done.title')}</h2>
      {#if setup?.accounts.length}
        <p>{t('welcome.done.you', { name: setup.accounts.map((a) => a.name).join(t('welcome.and')) })}</p>
      {/if}
      <p class="note">{t('welcome.done.backfill')}</p>
      <p class="note">{t('welcome.done.digest')}</p>
      <button type="button" class="choose primary" onclick={finish}>{t('welcome.done.open')}</button>
    </div>
  {/if}
</section>
