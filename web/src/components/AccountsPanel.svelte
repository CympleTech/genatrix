<script lang="ts">
  import { get, post } from '../lib/api';
  import { t } from '../lib/i18n';
  import Icon from './Icon.svelte';

  // Design 09 ("先落地的是设置页") and design 05 ("在界面上接入"): add a
  // mailbox, sign in to Telegram in three steps, sign in to Lark with a
  // link confirmed on any device ("飞书"), remove an account. Design 02,
  // invariant 12: only on the machine Genatrix runs on; a paired phone sees
  // the accounts and is told where to go.
  interface AccountView {
    kind: 'mail' | 'telegram' | 'lark'; id: string; name: string; detail: string;
    can_send: boolean; state: string; text: string; workspace?: string;
  }
  interface SetupView { local: boolean; accounts: AccountView[]; telegram: boolean }

  let view = $state<SetupView | null>(null);
  let error = $state('');
  type Adding = '' | 'mail' | 'telegram' | 'lark';
  let adding = $state<Adding>('');
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

  // lark: the app, then a link to confirm while this page asks Lark
  let larkBrand = $state<'lark' | 'feishu'>('lark');
  let appId = $state('');
  let appSecret = $state('');
  let larkLink = $state<{ url: string; code: string } | null>(null);
  let larkTimer: ReturnType<typeof setTimeout> | null = null;

  function stopLark() {
    if (larkTimer) clearTimeout(larkTimer);
    larkTimer = null; larkLink = null;
  }
  $effect(() => stopLark);

  async function larkStart(e: Event) {
    e.preventDefault();
    busy = true; formError = ''; stopLark();
    try {
      const r = await post<{ flow: string; url: string; code: string; interval: number }>(
        '/api/setup/lark/start', { app_id: appId, app_secret: appSecret, brand: larkBrand });
      appSecret = '';
      larkLink = { url: r.url, code: r.code };
      const every = Math.max(r.interval, 2) * 1000;
      const poll = async () => {
        try {
          const p = await post<{ waiting?: boolean; done?: boolean; name?: string }>('/api/setup/lark/poll', { flow: r.flow });
          if (p.done) {
            stopLark(); notice = t('accounts.added', { name: p.name ?? 'Lark' }); adding = ''; await load();
            return;
          }
          larkTimer = setTimeout(poll, every);
        } catch (err: any) { formError = err.message; stopLark(); }
      };
      larkTimer = setTimeout(poll, every);
    } catch (err: any) { formError = err.message; }
    busy = false;
  }

  async function load() {
    try { view = await get<SetupView>('/api/setup'); error = ''; } catch (e: any) { error = e.message; }
  }
  $effect(() => {
    load();
    const timer = setInterval(load, 5000);
    return () => clearInterval(timer);
  });

  function open(kind: Exclude<Adding, ''>) {
    adding = adding === kind ? '' : kind;
    stopLark(); appId = ''; appSecret = '';
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
          <span class="account-icon"><Icon name={a.kind === 'lark' ? 'task' : a.kind} size={18} /></span>
          <span class="account-body">
            <span class="account-name">{a.name}{#if a.workspace}<span class="role space-chip">{a.workspace}</span>{/if}</span>
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
        <button type="button" class="choose" class:is-on={adding === 'lark'} onclick={() => open('lark')}>
          <Icon name="task" size={16} />{t('accounts.addLark')}
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

      {#if adding === 'lark'}
        <form class="setup-form" onsubmit={larkStart}>
          {#if !larkLink}
            <div class="field">
              <span class="field-label">{t('accounts.lark.where')}</span>
              <div class="chooser">
                <button type="button" class="choose" class:is-on={larkBrand === 'lark'} onclick={() => (larkBrand = 'lark')}>{t('accounts.lark.lark')}</button>
                <button type="button" class="choose" class:is-on={larkBrand === 'feishu'} onclick={() => (larkBrand = 'feishu')}>{t('accounts.lark.feishu')}</button>
              </div>
            </div>
            <div class="help">
              <p class="help-title">{t('accounts.lark.help.title')}</p>
              <ol>
                <li>{t('accounts.lark.help.1')} <a href={larkBrand === 'lark' ? 'https://open.larksuite.com/app' : 'https://open.feishu.cn/app'} target="_blank" rel="noreferrer">{larkBrand === 'lark' ? 'open.larksuite.com/app' : 'open.feishu.cn/app'}</a></li>
                <li>{t('accounts.lark.help.2')}</li>
                <li>{t('accounts.lark.help.3')}</li>
              </ol>
            </div>
            <label>{t('accounts.lark.appId')}
              <input bind:value={appId} placeholder="cli_..." autocomplete="off" required />
            </label>
            <label>{t('accounts.lark.appSecret')}
              <input type="password" bind:value={appSecret} autocomplete="off" required />
            </label>
            <p class="note">{t('accounts.lark.note')}</p>
          {:else}
            <p class="note">{t('accounts.lark.open')}</p>
            <p><a href={larkLink.url} target="_blank" rel="noreferrer">{larkLink.url}</a></p>
            <p class="note">{t('accounts.lark.code', { code: larkLink.code })}</p>
            <p class="note">{t('accounts.lark.waiting')}</p>
          {/if}
          {#if formError}<p class="note error">{formError}</p>{/if}
          <div class="chooser">
            {#if !larkLink}
              <button type="submit" class="choose primary" disabled={busy}>{busy ? t('accounts.working') : t('accounts.lark.start')}</button>
            {/if}
            <button type="button" class="choose lowers" onclick={() => open('lark')}>{t('accounts.cancel')}</button>
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
