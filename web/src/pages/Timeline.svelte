<script lang="ts">
  import { get, type Row } from '../lib/api';
  import { t } from '../lib/i18n';
  import { status } from '../lib/status';
  import ItemRow from '../components/ItemRow.svelte';
  import Empty from '../components/Empty.svelte';

  let q = $state('');
  let level = $state('');
  let connector = $state('');
  let rows = $state<Row[] | null>(null);
  let error = $state('');
  let typing: ReturnType<typeof setTimeout> | undefined;
  let knownItems: number | null = null;

  async function load() {
    const params = new URLSearchParams({ q: q.trim(), level, connector });
    try { rows = await get<Row[]>('/api/timeline?' + params); error = ''; }
    catch (e: any) { error = e.message; rows = []; }
  }
  function typed() { clearTimeout(typing); typing = setTimeout(load, 150); }
  $effect(() => { load(); });
  // New mail shows up without a reload.
  $effect(() => {
    const items = $status?.items ?? null;
    if (items !== null && knownItems !== null && items !== knownItems) load();
    knownItems = items;
  });
</script>

<section class="pane">
  <form class="filters" onsubmit={(e) => e.preventDefault()}>
    <input type="search" bind:value={q} oninput={typed} placeholder={t('timeline.search')} autocomplete="off" />
    <select bind:value={connector} onchange={load}>
      <option value="">{t('timeline.all')}</option>
      <option value="imap">{t('connector.imap')}</option>
      <option value="telegram">{t('connector.telegram')}</option>
    </select>
    <select bind:value={level} onchange={load}>
      <option value="">{t('timeline.levels')}</option>
      <option value="public">{t('level.public')}</option>
      <option value="personal">{t('level.personal')}</option>
      <option value="secret">{t('level.secret')}</option>
    </select>
  </form>
  {#if error}<Empty text={error} error />
  {:else if !rows}<Empty text={t('loading')} />
  {:else if !rows.length}<Empty text={q.trim() ? t('timeline.nomatch') : t('timeline.empty')} />
  {:else}
    <ol class="rows panel">{#each rows as row (row.id)}<ItemRow {row} />{/each}</ol>
  {/if}
</section>
