<script lang="ts">
  import { get, post } from '../lib/api';
  import { t } from '../lib/i18n';
  import { bytes } from '../lib/format';

  // Design 04, "模型下载": the name, the size, whether the disk can take it,
  // one button, progress, resumable, every file checked against its pinned
  // hash before it is used.
  interface ModelView { id: string; role: string; repo: string; size: number; present: number }
  interface Progress { running: boolean; current: string; step: string; done: number; total: number; error: string | null }
  interface ModelPage {
    local: boolean; models: ModelView[]; total: number; present: number; free: number | null;
    state: string; ready: boolean; download: Progress;
  }

  let page = $state<ModelPage | null>(null);
  let error = $state('');

  async function load() {
    try { page = await get<ModelPage>('/api/model'); error = ''; } catch (e: any) { error = e.message; }
  }
  // Every second while a download runs, every five otherwise.
  $effect(() => {
    let alive = true;
    const loop = async () => {
      while (alive) {
        await load();
        await new Promise((r) => setTimeout(r, page?.download.running ? 1000 : 5000));
      }
    };
    loop();
    return () => { alive = false; };
  });

  async function download() {
    error = '';
    try { await post('/api/model/download', {}); await load(); } catch (e: any) { error = e.message; }
  }

  const missing = $derived(page ? page.total - page.present : 0);
  const need = $derived(missing + Math.floor(missing / 10));
  const roomy = $derived(!page || page.free == null || page.free >= need);
  const pct = $derived(page && page.download.total ? Math.floor((page.download.done / page.download.total) * 100) : 0);
  const roleName = (r: string) => t(`models.role.${r}`);
</script>

<div class="section models-panel">
  <h2 class="group-title">{t('settings.model')}</h2>
  {#if page}
    <p class="model-state" class:ready={page.ready}>
      <span class="dot"></span>{page.state}
    </p>
    <ul class="model-list">
      {#each page.models as m (m.id)}
        <li class="model-row">
          <span class="model-body">
            <span class="model-name">{roleName(m.role)}</span>
            <span class="faint">{m.repo}</span>
          </span>
          <span class="model-size">{bytes(m.size)}</span>
          <span class="model-status" class:ok={m.present === m.size}>
            {m.present === m.size ? t('models.installed') : m.present > 0 ? t('models.partial', { n: bytes(m.size - m.present) }) : t('models.missing')}
          </span>
        </li>
      {/each}
    </ul>

    {#if page.download.running}
      <div class="progress" role="progressbar" aria-valuenow={pct} aria-valuemin="0" aria-valuemax="100">
        <span style="width:{pct}%"></span>
      </div>
      <p class="note">
        {pct}% · {bytes(page.download.done)} / {bytes(page.download.total)} ·
        {page.download.step === 'checking' ? t('models.checking') : t('models.fetching')} {page.download.current}
      </p>
    {:else if missing > 0}
      {#if page.download.error}<p class="note error">{page.download.error}</p>{/if}
      {#if page.local}
        <div class="chooser">
          <button type="button" class="choose primary" disabled={!roomy} onclick={download}>
            {page.present > 0 ? t('models.resume', { n: bytes(missing) }) : t('models.download', { n: bytes(missing) })}
          </button>
          {#if page.free != null}<span class="note">{t('models.free', { n: bytes(page.free) })}</span>{/if}
        </div>
        {#if !roomy}<p class="note error">{t('models.noRoom', { need: bytes(need), free: bytes(page.free ?? 0) })}</p>{/if}
        <p class="note">{t('models.note')}</p>
      {:else}
        <p class="note">{t('models.onlyHere')}</p>
      {/if}
    {/if}
  {/if}
  {#if error}<p class="note error">{error}</p>{/if}
</div>
