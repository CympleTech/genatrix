<script lang="ts">
  import { get, post } from '../lib/api';
  import { t } from '../lib/i18n';
  import Icon from './Icon.svelte';

  // Design 09 ("先落地的是设置页") and design 05 ("在界面上接入"): add a
  // mailbox, sign in to Telegram in three steps, remove an account. Design 02,
  // invariant 12: only on the machine Genatrix runs on; a paired phone sees
  // the accounts and is told where to go.
  interface AccountView {
    kind: 'mail' | 'telegram'; id: string; name: string; detail: string;
    can_send: boolean; state: string; text: string;
  }
  interface SetupView { local: boolean; accounts: AccountView[]; telegram: boolean }

  let view = $state<SetupView | null>(null);
  let error = $state('');
  let adding = $state<'' | 'mail' | 'telegram'>('');
  let confirming = $state('');
  let notice = $state('');

  // mail form
  let address = $state('');
  let password = $state('');
  let imapHost = $state('');
  let needsServer = $state(false);
  let busy = $state(false);
  let formError = $state('');

  // telegram steps
  let tgStep = $state<'phone' | 'code' | 'password'>('phone');
  let phone = $state('');
  let code = $state('');
  let tgPassword = $state('');
  let hint = $state<string | null>(null);
  let flow = $state('');

  async function load() {
    try { view = await get<SetupView>('/api/setup'); error = ''; } catch (e: any) { error = e.message; }
  }
  $effect(() => {
    load();
    const timer = setInterval(load, 5000);
    return () => clearInterval(timer);
  });

  function open(kind: 'mail' | 'telegram') {
    adding = adding === kind ? '' : kind;
    formError = ''; notice = ''; busy = false;
    address = ''; password = ''; imapHost = ''; needsServer = false;
    tgStep = 'phone'; phone = ''; code = ''; tgPassword = ''; hint = null; flow = '';
  }

  async function addMail(e: Event) {
    e.preventDefault();
    busy = true; formError = '';
    try {
      await post('/api/setup/mail', { address, password, imap_host: needsServer ? imapHost : null });
      notice = t('accounts.added', { name: address.trim() });
      adding = '';
      await load();
    } catch (err: any) {
      formError = err.message;
      if (/IMAP server/i.test(err.message)) needsServer = true;
    }
    busy = false;
    password = '';
  }

  async function telegramStep(e: Event) {
    e.preventDefault();
    busy = true; formError = '';
    try {
      if (tgStep === 'phone') {
        const r = await post<{ flow: string }>('/api/setup/telegram/start', { phone });
        flow = r.flow; tgStep = 'code';
      } else if (tgStep === 'code') {
        const r = await post<{ done?: boolean; name?: string; password?: boolean; hint?: string | null }>(
          '/api/setup/telegram/code', { flow, code });
        if (r.password) { tgStep = 'password'; hint = r.hint ?? null; }
        else { notice = t('accounts.added', { name: r.name ?? phone }); adding = ''; await load(); }
      } else {
        const r = await post<{ name: string }>('/api/setup/telegram/password', { flow, password: tgPassword });
        notice = t('accounts.added', { name: r.name ?? phone }); adding = ''; await load();
      }
    } catch (err: any) {
      formError = err.message;
      if (err.status === 410) tgStep = 'phone';
    }
    busy = false;
    tgPassword = '';
  }

  async function remove(a: AccountView) {
    if (confirming !== a.kind + a.id) { confirming = a.kind + a.id; return; }
    confirming = '';
    try {
      await post('/api/setup/remove', { kind: a.kind, id: a.id });
      notice = t('accounts.removed', { name: a.name });
      await load();
    } catch (e: any) { error = e.message; }
  }

  const gmail = $derived(/@(gmail|googlemail)\.com\s*$/i.test(address) || address.trim() === '');
  const stateClass = (s: string) => (s === 'live' ? 'ok' : s === 'needs_login' || s === 'stopped' ? 'bad' : 'busy');
</script>

<div class="section accounts-panel">
  <h2 class="group-title">{t('settings.accounts')}</h2>
  {#if error}<p class="note error">{error}</p>{/if}
  {#if notice}<p class="note ok-note">{notice}</p>{/if}

  {#if view}
    {#if !view.accounts.length}<p class="note">{t('accounts.none')}</p>{/if}
    <ul class="account-list">
      {#each view.accounts as a (a.kind + a.id)}
        <li class="account-row">
          <span class="account-icon"><Icon name={a.kind === 'mail' ? 'mail' : 'telegram'} size={18} /></span>
          <span class="account-body">
            <span class="account-name">{a.name}</span>
            <span class="account-state {stateClass(a.state)}">{a.text}</span>
          </span>
          {#if view.local}
            <button type="button" class="choose lowers" onclick={() => remove(a)}>
              {confirming === a.kind + a.id ? t('accounts.confirmRemove') : t('accounts.remove')}
            </button>
          {/if}
        </li>
      {/each}
    </ul>

    {#if !view.local}
      <p class="note">{t('accounts.onlyHere')}</p>
    {:else}
      <div class="chooser">
        <button type="button" class="choose" class:is-on={adding === 'mail'} onclick={() => open('mail')}>
          <Icon name="mail" size={16} />{t('accounts.addMail')}
        </button>
        <button type="button" class="choose" class:is-on={adding === 'telegram'} disabled={!view.telegram} onclick={() => open('telegram')}>
          <Icon name="telegram" size={16} />{t('accounts.addTelegram')}
        </button>
      </div>
      {#if !view.telegram}<p class="note">{t('accounts.noTelegramCredentials')}</p>{/if}

      {#if adding === 'mail'}
        <form class="setup-form" onsubmit={addMail}>
          <label>{t('accounts.address')}
            <input type="email" bind:value={address} autocomplete="email" placeholder="you@gmail.com" required />
          </label>
          <label>{t('accounts.appPassword')}
            <input type="password" bind:value={password} autocomplete="off" required />
          </label>
          {#if needsServer}
            <label>{t('accounts.imapHost')}
              <input bind:value={imapHost} placeholder="imap.example.com" autocomplete="off" required />
            </label>
          {/if}
          {#if gmail}
            <div class="help">
              <p class="help-title">{t('accounts.gmailHelp.title')}</p>
              <ol>
                <li>{t('accounts.gmailHelp.1')}</li>
                <li>{t('accounts.gmailHelp.2')} <a href="https://myaccount.google.com/apppasswords" target="_blank" rel="noreferrer">myaccount.google.com/apppasswords</a></li>
                <li>{t('accounts.gmailHelp.3')}</li>
              </ol>
            </div>
          {/if}
          <p class="note">{t('accounts.mailNote')}</p>
          {#if formError}<p class="note error">{formError}</p>{/if}
          <div class="chooser">
            <button type="submit" class="choose primary" disabled={busy}>{busy ? t('accounts.checking') : t('accounts.checkAndAdd')}</button>
            <button type="button" class="choose lowers" onclick={() => open('mail')}>{t('accounts.cancel')}</button>
          </div>
        </form>
      {/if}

      {#if adding === 'telegram'}
        <form class="setup-form" onsubmit={telegramStep}>
          <ol class="steps">
            <li class:on={tgStep === 'phone'} class:done={tgStep !== 'phone'}>{t('accounts.tg.step1')}</li>
            <li class:on={tgStep === 'code'} class:done={tgStep === 'password'}>{t('accounts.tg.step2')}</li>
            <li class:on={tgStep === 'password'}>{t('accounts.tg.step3')}</li>
          </ol>
          {#if tgStep === 'phone'}
            <label>{t('accounts.tg.phone')}
              <input type="tel" bind:value={phone} placeholder="+86 138 0000 0000" autocomplete="tel" required />
            </label>
            <p class="note">{t('accounts.tg.phoneNote')}</p>
          {:else if tgStep === 'code'}
            <label>{t('accounts.tg.code')}
              <input bind:value={code} inputmode="numeric" autocomplete="one-time-code" required />
            </label>
            <p class="note">{t('accounts.tg.codeNote')}</p>
          {:else}
            <label>{t('accounts.tg.password')}{hint ? ` (${t('accounts.tg.hint', { hint })})` : ''}
              <input type="password" bind:value={tgPassword} autocomplete="off" required />
            </label>
          {/if}
          {#if formError}<p class="note error">{formError}</p>{/if}
          <div class="chooser">
            <button type="submit" class="choose primary" disabled={busy}>
              {busy ? t('accounts.working') : tgStep === 'phone' ? t('accounts.tg.send') : t('accounts.tg.signIn')}
            </button>
            <button type="button" class="choose lowers" onclick={() => open('telegram')}>{t('accounts.cancel')}</button>
          </div>
        </form>
      {/if}
    {/if}
  {/if}
</div>
