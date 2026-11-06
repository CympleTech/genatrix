<script lang="ts">
  import qrcode from 'qrcode-generator';
  import { get, post, type AdminView, type Device } from '../lib/api';
  import { languageChoice, setLanguage, t, type LangChoice } from '../lib/i18n';
  import { bytes } from '../lib/format';
  import { status } from '../lib/status';
  import Empty from '../components/Empty.svelte';
  import AccountsPanel from '../components/AccountsPanel.svelte';
  import ModelsPanel from '../components/ModelsPanel.svelte';
  import AgentsPanel from '../components/AgentsPanel.svelte';
  import JunkPanel from '../components/JunkPanel.svelte';

  let devices = $state<Device[] | null>(null);
  let error = $state('');
  let pairing = $state<{ code: string; expires_in_secs: number; reachable_at: string[] } | null>(null);
  let pairError = $state('');
  let qr = $state('');

  async function loadDevices() {
    try {
      [devices, admin] = await Promise.all([get<Device[]>('/api/devices'), get<AdminView>('/api/admin')]);
    } catch (e: any) { error = e.message; devices = devices ?? []; }
  }

  // Design 06 v0.7: admin devices. The role and the passphrase are set
  // here, on the machine Genatrix runs on; an admin device is asked for the
  // passphrase on each high-privilege change, and five wrong ones lock it.
  let admin = $state<AdminView | null>(null);
  let pass1 = $state('');
  let pass2 = $state('');
  let adminNote = $state('');
  let adminError = $state('');
  async function setPassphrase() {
    adminError = ''; adminNote = '';
    if (pass1 !== pass2) { adminError = t('admin.mismatch'); return; }
    try {
      await post('/api/admin/passphrase', { passphrase: pass1 });
      pass1 = ''; pass2 = '';
      adminNote = t('admin.set.done');
      await loadDevices();
    } catch (e: any) { adminError = e.message; }
  }
  async function unlock() {
    adminError = '';
    try { await post('/api/admin/unlock', {}); await loadDevices(); } catch (e: any) { adminError = e.message; }
  }
  async function role(d: Device, on: boolean) {
    error = '';
    try { await post(`/api/device/${d.id}/admin`, { admin: on }); await loadDevices(); } catch (e: any) { error = e.message; }
  }
  async function startPairing() {
    pairError = '';
    try {
      pairing = await post('/api/pair/start', {});
      // The core says where a phone can reach it; this page is usually on
      // 127.0.0.1, which on the phone would mean the phone.
      const origin = pairing!.reachable_at[0];
      if (!origin) { qr = ''; return; }
      const url = `${origin}/pair?code=${pairing!.code}`;
      const q = qrcode(0, 'M');
      q.addData(url); q.make();
      qr = q.createSvgTag({ cellSize: 4, margin: 2, scalable: true });
    } catch (e: any) { pairError = e.status === 403 && !e.body?.need && !e.body?.locked ? t('settings.pair.local') : e.message; }
  }
  async function revoke(id: string) {
    try { await post(`/api/device/${id}/revoke`, {}); await loadDevices(); } catch (e: any) { error = e.message; }
  }
  $effect(() => { loadDevices(); });

  let exporting = $state(false);
  let exported = $state('');
  let erasing = $state(false);
  let erased = $state(false);
  let busyErase = $state(false);
  let eraseError = $state('');
  let word = $state('');
  const wordOk = $derived(word.trim() === '删除' || word.trim().toUpperCase() === 'DELETE');

  async function exportAll() {
    exporting = true; exported = '';
    try { exported = (await post<{ path: string }>('/api/export', {})).path; }
    catch (e: any) { error = e.message; }
    exporting = false;
  }
  async function eraseAll() {
    busyErase = true; eraseError = '';
    try { await post('/api/erase', { confirm: word }); erased = true; }
    catch (e: any) { eraseError = e.message; }
    busyErase = false;
  }
  const pairUrl = $derived(pairing?.reachable_at[0] ? `${pairing.reachable_at[0]}/pair` : '');
</script>

<section class="pane settings">
  <div class="section">
    <h2 class="group-title">{t('settings.language')}</h2>
    <div class="chooser">
      {#each ['auto', 'zh', 'en'] as const as choice}
        <button type="button" class="choose" class:is-on={languageChoice() === choice}
          onclick={() => { if (languageChoice() !== choice) setLanguage(choice as LangChoice); }}>
          {t(`settings.language.${choice}`)}
        </button>
      {/each}
    </div>
    <p class="note">{t('settings.language.note')}</p>
  </div>
  <AccountsPanel />
  <ModelsPanel />
  <AgentsPanel />
  <JunkPanel />

  <div class="section">
  <h2 class="group-title">{t('settings.devices')}</h2>
  <p class="note">{t('settings.devices.note')}</p>
  {#if error}<Empty text={error} error />{/if}
  {#if devices && !devices.length}<p class="note">{t('settings.nodevices')}</p>{/if}
  <ul class="devices">
    {#each devices ?? [] as d (d.id)}
      <li class="device" class:revoked={!!d.revoked_at}>
        <span class="device-name">{d.name}{d.this ? ` · ${t('settings.thisdevice')}` : ''}{#if d.admin}<span class="role">{t('admin.role')}</span>{/if}</span>
        <span class="faint">{d.revoked_at ? t('settings.revoked', { at: d.revoked_at.slice(0, 16).replace('T', ' ') }) : d.last_seen ? t('settings.lastseen', { at: d.last_seen.slice(0, 16).replace('T', ' ') }) : t('settings.neverseen')}</span>
        {#if !d.revoked_at && admin?.here}
          {#if d.admin}<button type="button" class="choose lowers" onclick={() => role(d, false)}>{t('admin.ungrant')}</button>
          {:else}<button type="button" class="choose" disabled={!admin.passphrase_set} onclick={() => role(d, true)}>{t('admin.grant')}</button>{/if}
        {/if}
        {#if !d.revoked_at}<button type="button" class="choose lowers" onclick={() => revoke(d.id)}>{t('settings.revoke')}</button>{/if}
      </li>
    {/each}
  </ul>
  {#if admin}
    <h3 class="group-title">{t('admin.title')}</h3>
    {#if admin.locked}
      <p class="note error">{t('admin.locked')}</p>
      {#if admin.here}<div class="chooser"><button type="button" class="choose primary" onclick={unlock}>{t('admin.unlock')}</button></div>{/if}
    {/if}
    {#if admin.here}
      <p class="note">{admin.passphrase_set ? t('admin.note.here.set') : t('admin.note.here.unset')}</p>
      <div class="setup-form">
        <label>{admin.passphrase_set ? t('admin.change') : t('admin.passphrase')}<input type="password" autocomplete="new-password" bind:value={pass1} /></label>
        <label>{t('admin.again')}<input type="password" autocomplete="new-password" bind:value={pass2} /></label>
        <div class="chooser"><button type="button" class="choose" disabled={pass1.length < 10 || !pass2} onclick={setPassphrase}>{admin.passphrase_set ? t('admin.change.go') : t('admin.set.go')}</button></div>
        <p class="field-note">{t('admin.length')}</p>
      </div>
    {:else if admin.admin}
      <p class="note">{t('admin.note.admin')}</p>
    {:else}
      <p class="note">{t('admin.note.ordinary')}</p>
    {/if}
    {#if adminError}<p class="note error">{adminError}</p>{/if}
    {#if adminNote}<p class="note ok-note">{adminNote}</p>{/if}
  {/if}
  <div class="pairing">
    <button type="button" class="choose primary" onclick={startPairing}>{t('settings.pair')}</button>
    {#if pairError}<span class="error">{pairError}</span>{/if}
    {#if pairing}
      <p class="note">{t('settings.pair.code')}</p>
      {#if pairUrl}<p class="pair-url">{pairUrl}</p>{:else}<p class="note error">{t('settings.pair.unreachable')}</p>{/if}
      <p class="pair-code">{pairing.code}</p>
      <div class="qr">{@html qr}</div>
      <p class="note">{t('settings.pair.expires')} {t('settings.pair.bindnote')}</p>
    {/if}
  </div>
  </div>

  <div class="section">
  <h2 class="group-title">{t('settings.data')}</h2>
  {#if $status}
    <dl class="facts">
      <dt>{t('settings.datadir')}</dt><dd>{$status.data_dir}</dd>
      <dt>{t('settings.ondisk')}</dt><dd>{bytes($status.bytes_on_disk)}</dd>
      <dt>{t('status.items', { n: '' }).trim()}</dt><dd>{$status.items}</dd>
      <dt>{t('settings.embedded')}</dt><dd>{$status.embedded}</dd>
      <dt>{t('settings.summarized')}</dt><dd>{$status.summarized}</dd>
      <dt>{t('settings.ledger')}</dt><dd>{$status.ledger_entries}</dd>
      <dt>{t('settings.rules')}</dt><dd>{$status.rules_version}</dd>
    </dl>
  {/if}
  </div>

  <!-- Design 09, "导出" and "卸载"; design 10: the friend is told before
       anything starts that all of it can be deleted at any time. -->
  <div class="section danger-zone">
    <h2 class="group-title">{t('erase.title')}</h2>
    <p class="note">{t('erase.exportNote')}</p>
    <div class="chooser">
      <button type="button" class="choose" disabled={exporting} onclick={exportAll}>{exporting ? t('erase.exporting') : t('erase.export')}</button>
      {#if exported}<span class="note ok-note">{t('erase.exported', { path: exported })}</span>{/if}
    </div>
    <p class="note">{t('erase.note')}</p>
    {#if !erasing}
      <button type="button" class="choose danger" onclick={() => (erasing = true)}>{t('erase.start')}</button>
    {:else if erased}
      <div class="erased">
        <p class="headline small">{t('erase.done.title')}</p>
        <p class="note">{t('erase.done.body')}</p>
      </div>
    {:else}
      <div class="setup-form">
        <p class="note">{t('erase.confirmNote')}</p>
        <label>{t('erase.word')}<input bind:value={word} autocomplete="off" /></label>
        {#if eraseError}<p class="note error">{eraseError}</p>{/if}
        <div class="chooser">
          <button type="button" class="choose danger" disabled={!wordOk || busyErase} onclick={eraseAll}>{busyErase ? t('erase.deleting') : t('erase.confirm')}</button>
          <button type="button" class="choose lowers" onclick={() => { erasing = false; word = ''; }}>{t('accounts.cancel')}</button>
        </div>
      </div>
    {/if}
  </div>
</section>
