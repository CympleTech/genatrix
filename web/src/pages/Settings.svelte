<script lang="ts">
  import qrcode from 'qrcode-generator';
  import { get, post, type Device } from '../lib/api';
  import { t } from '../lib/i18n';
  import { bytes } from '../lib/format';
  import { status, accounts } from '../lib/status';
  import Empty from '../components/Empty.svelte';

  let devices = $state<Device[] | null>(null);
  let error = $state('');
  let pairing = $state<{ code: string; expires_in_secs: number } | null>(null);
  let pairError = $state('');
  let qr = $state('');

  async function loadDevices() {
    try { devices = await get<Device[]>('/api/devices'); } catch (e: any) { error = e.message; devices = []; }
  }
  async function startPairing() {
    pairError = '';
    try {
      pairing = await post('/api/pair/start', {});
      const url = `${location.origin}/pair?code=${pairing!.code}`;
      const q = qrcode(0, 'M');
      q.addData(url); q.make();
      qr = q.createSvgTag({ cellSize: 4, margin: 2, scalable: true });
    } catch (e: any) { pairError = e.status === 403 ? t('settings.pair.local') : e.message; }
  }
  async function revoke(id: string) {
    try { await post(`/api/device/${id}/revoke`, {}); await loadDevices(); } catch (e: any) { error = e.message; }
  }
  $effect(() => { loadDevices(); });
  const pairUrl = $derived(pairing ? `${location.origin}/pair` : '');
</script>

<section class="pane settings">
  <h2 class="group-title">{t('settings.accounts')}</h2>
  {#if !$accounts.length}<p class="note">{t('settings.noaccounts')}</p>{/if}
  <ul class="accounts">
    {#each $accounts as a}
      <li class="account {a.sync.state}"><span class="address">{a.address}</span><span class="state">{a.text}</span></li>
    {/each}
  </ul>

  <h2 class="group-title">{t('settings.model')}</h2>
  <p class="note">{$status?.model_text ?? t('loading')} · {$status?.cloud_enabled ? t('status.cloud.on') : t('status.cloud.off')}</p>

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
      <p class="pair-url">{pairUrl}</p>
      <p class="pair-code">{pairing.code}</p>
      <div class="qr">{@html qr}</div>
      <p class="note">{t('settings.pair.expires')} {t('settings.pair.bindnote')}</p>
    {/if}
  </div>

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
</section>
