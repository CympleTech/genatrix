<script lang="ts">
  import qrcode from 'qrcode-generator';
  import { get, post, type Device } from '../lib/api';
  import { t } from '../lib/i18n';
  import { bytes } from '../lib/format';
  import { status } from '../lib/status';
  import Empty from '../components/Empty.svelte';
  import AccountsPanel from '../components/AccountsPanel.svelte';
  import ModelsPanel from '../components/ModelsPanel.svelte';

  let devices = $state<Device[] | null>(null);
  let error = $state('');
  let pairing = $state<{ code: string; expires_in_secs: number; reachable_at: string[] } | null>(null);
  let pairError = $state('');
  let qr = $state('');

  async function loadDevices() {
    try { devices = await get<Device[]>('/api/devices'); } catch (e: any) { error = e.message; devices = []; }
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
    } catch (e: any) { pairError = e.status === 403 ? t('settings.pair.local') : e.message; }
  }
  async function revoke(id: string) {
    try { await post(`/api/device/${id}/revoke`, {}); await loadDevices(); } catch (e: any) { error = e.message; }
  }
  $effect(() => { loadDevices(); });
  const pairUrl = $derived(pairing?.reachable_at[0] ? `${pairing.reachable_at[0]}/pair` : '');
</script>

<section class="pane settings">
  <AccountsPanel />
  <ModelsPanel />

  <div class="section">
  <h2 class="group-title">{t('settings.devices')}</h2>
  <p class="note">{t('settings.devices.note')}</p>
  {#if error}<Empty text={error} error />{/if}
  {#if devices && !devices.length}<p class="note">{t('settings.nodevices')}</p>{/if}
  <ul class="devices">
    {#each devices ?? [] as d (d.id)}
      <li class="device" class:revoked={!!d.revoked_at}>
        <span class="device-name">{d.name}{d.this ? ` · ${t('settings.thisdevice')}` : ''}</span>
        <span class="faint">{d.revoked_at ? t('settings.revoked', { at: d.revoked_at.slice(0, 16).replace('T', ' ') }) : d.last_seen ? t('settings.lastseen', { at: d.last_seen.slice(0, 16).replace('T', ' ') }) : t('settings.neverseen')}</span>
        {#if !d.revoked_at}<button type="button" class="choose lowers" onclick={() => revoke(d.id)}>{t('settings.revoke')}</button>{/if}
      </li>
    {/each}
  </ul>
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
</section>
