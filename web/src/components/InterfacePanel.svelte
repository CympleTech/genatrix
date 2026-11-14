<script lang="ts">
  import { get, post } from '../lib/api';
  import { t } from '../lib/i18n';
  import { go } from '../lib/router';

  // Design 15, "每台设备选一个界面": the shell's own panel. Which interface
  // this device uses; follow another device's, copy one to change it, look
  // at a new version before approving it, go back to an older one.
  let { onchosen, onclose, onreview }: { onchosen: () => void; onclose: () => void; onreview: (action: string) => void } = $props();

  interface Version { hash: string; note: string; created_at: string; approved_at: string | null; current: boolean }
  interface Iface { name: string; title: string; builtin: boolean; made_by: string; devices: string[]; versions: Version[]; project?: string | null }
  interface Dev { id: string; name: string; ui: string; this: boolean }
  interface State { device: string; using: string; chosen: boolean; uis: Iface[]; devices: Dev[]; deps_ready: boolean }

  let s = $state<State | null>(null);
  let error = $state('');
  let notice = $state('');
  let copying = $state('');
  let copyTitle = $state('');
  let busy = $state(false);

  async function load() {
    try { s = await get<State>('/api/ui'); } catch (e: any) { error = e.message; }
  }
  $effect(() => { load(); });

  async function act(fn: () => Promise<unknown>, done?: string) {
    busy = true; error = ''; notice = '';
    try { await fn(); if (done) notice = done; await load(); }
    catch (e: any) { error = e.message; }
    busy = false;
  }
  const use = (name: string) => act(async () => { await post('/api/ui/choose', { ui: name }); onchosen(); });
  const copy = (from: string) => act(async () => {
    const r = await post<{ name: string }>('/api/ui/copy', { from, title: copyTitle.trim() });
    copying = ''; copyTitle = '';
    await post('/api/ui/choose', { ui: r.name });
    onchosen();
  });
  // Design 15, "让 AI 改界面": a change in words becomes a work card on the
  // interface's source; its build comes back here as a version to approve.
  let asking = $state('');
  let ask = $state('');
  const change = (ui: string) => act(async () => {
    const r = await post<{ action: string }>('/api/ui/change', { ui, task: ask.trim() });
    asking = ''; ask = '';
    onreview(r.action);
  });
  const approve = (ui: string, hash: string) => act(() => post('/api/ui/approve', { ui, hash }), t('ui.approved'));
  const devName = (id: string) => s?.devices.find((d) => d.id === id)?.name || (id === 'local' ? t('ui.thismachine') : id);
  const when = (at: string | null) => (at ? new Date(at).toLocaleString() : '');
</script>

<div class="interface-panel">
  {#if error}<p class="note error">{error}</p>{/if}
  {#if notice}<p class="note">{notice}</p>{/if}
  {#if !s}<p class="note">{t('loading')}</p>
  {:else}
    {#if !s.chosen}<p class="note">{t('ui.first')}</p>{/if}
    <p class="note">{t('ui.lede')}</p>
    <ul class="rows">
      {#each s.uis as u (u.name)}
        <li class="row ui-row" class:is-on={s.using === u.name}>
          <div class="project-line">
            <span>
              <span class="who">{u.builtin ? t('ui.default') : u.title}</span>
              {#if s.using === u.name}<span class="badge-soft on">{t('ui.here')}</span>{/if}
              <span class="faint"> · {u.devices.length ? t('ui.usedby', { who: u.devices.map(devName).join(', ') }) : t('ui.unused')}</span>
            </span>
            <span class="chooser">
              {#if s.using !== u.name && (u.builtin || u.versions.some((v) => v.current))}
                <button type="button" class="choose primary" disabled={busy} onclick={() => use(u.name)}>{u.devices.length && !u.builtin ? t('ui.follow') : t('ui.use')}</button>
              {/if}
              <button type="button" class="choose" disabled={busy} onclick={() => { copying = u.name; copyTitle = ''; }}>{t('ui.copy')}</button>
            </span>
          </div>
          {#if copying === u.name}
            <div class="setup-form">
              <label>{t('ui.copy.title')}<input bind:value={copyTitle} maxlength="40" placeholder={t('ui.copy.eg')} /></label>
              <div class="chooser">
                <button type="button" class="choose primary" disabled={busy || !copyTitle.trim()} onclick={() => copy(u.name)}>{t('ui.copy.go')}</button>
                <button type="button" class="choose lowers" onclick={() => (copying = '')}>{t('accounts.cancel')}</button>
              </div>
              <p class="field-note">{t('ui.copy.note')}</p>
            </div>
          {/if}
          {#if !u.builtin}
            {#if u.project}
              {#if asking === u.name}
                <div class="setup-form">
                  <label>{t('ui.change.what')}<textarea class="draft" bind:value={ask} placeholder={t('ui.change.eg')}></textarea></label>
                  <p class="field-note">{t('ui.change.note')}</p>
                  {#if !s.deps_ready}<p class="field-note error">{t('ui.change.deps')} <code>genatrix ui deps</code></p>{/if}
                  <div class="chooser">
                    <button type="button" class="choose primary" disabled={busy || !ask.trim()} onclick={() => change(u.name)}>{t('ui.change.go')}</button>
                    <button type="button" class="choose lowers" onclick={() => (asking = '')}>{t('accounts.cancel')}</button>
                  </div>
                </div>
              {:else}
                <div class="chooser"><button type="button" class="choose" onclick={() => { asking = u.name; ask = ''; }}>{t('ui.change')}</button></div>
              {/if}
            {:else}
              <p class="field-note">{t('ui.change.nosource')}</p>
            {/if}
            <ol class="ui-versions">
              {#each u.versions as v (v.hash)}
                <li>
                  <span class="faint">{when(v.created_at)}{v.note ? ` · ${v.note}` : ''}</span>
                  {#if v.current}<span class="badge-soft on">{t('ui.current')}</span>
                  {:else if !v.approved_at}<span class="badge-soft">{t('ui.pending')}</span>{/if}
                  <span class="chooser">
                    <button type="button" class="choose lowers" onclick={() => { go('/?preview=' + v.hash); onclose(); }}>{t('ui.preview')}</button>
                    {#if !v.current}
                      <button type="button" class="choose" disabled={busy} onclick={() => approve(u.name, v.hash)}>{v.approved_at ? t('ui.rollback') : t('ui.approve')}</button>
                    {/if}
                  </span>
                </li>
              {/each}
            </ol>
            {#if u.devices.some((d) => d !== s?.device)}<p class="field-note">{t('ui.approve.others')}</p>{/if}
          {/if}
        </li>
      {/each}
    </ul>
    <p class="field-note">{t('ui.safe.note')} <code>/safe</code></p>
  {/if}
</div>
