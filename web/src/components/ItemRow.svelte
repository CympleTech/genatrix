<script lang="ts">
  import { get, post, type Row, type Detail } from '../lib/api';
  import { connectorName } from '../lib/format';
  import { t } from '../lib/i18n';
  import LevelMark from './LevelMark.svelte';
  import LevelChooser from './LevelChooser.svelte';
  import { refreshPending } from '../lib/status';

  let { row: initial }: { row: Row } = $props();
  // The row changes when the user changes its level; the list does not.
  // svelte-ignore state_referenced_locally
  let row = $state(initial);
  let open = $state(false);
  let detail = $state<Detail | null>(null);
  let error = $state('');
  let drafting = $state<'' | 'busy' | 'done' | 'failed'>('');
  let draftError = $state('');

  async function toggle() {
    open = !open;
    if (open && !detail) {
      try { detail = await get<Detail>(`/api/item/${row.id}`); } catch (e: any) { error = e.message; }
    }
  }
  async function draft() {
    drafting = 'busy';
    try { await post(`/api/item/${row.id}/draft`, {}); drafting = 'done'; refreshPending(); }
    catch (e: any) { drafting = 'failed'; draftError = e.message; }
  }
  function levelChanged(d: Detail) {
    row = d.row;
    detail = d;
    open = false;
  }
  const replyable = $derived(row.direction === 'inbound' && (row.connector === 'imap' || row.connector === 'telegram'));
</script>

<li class="row" class:is-open={open}>
  <div class="meta">
    <time>{row.at}</time>
    <LevelMark level={row.level} reason={row.level_reason} />
    <span class="who">{row.author}</span>
    <span>{connectorName(row.connector)}</span>
    {#if row.thread && row.thread !== row.author}<span class="thread">{row.thread}</span>{/if}
  </div>
  <button type="button" class="preview" onclick={toggle}>{row.preview}{row.has_more ? '…' : ''}</button>
  {#if open}
    <div class="detail">
      {#if error}<p class="error">{error}</p>
      {:else if !detail}<p>{t('loading')}</p>
      {:else}
        {#if detail.subject}<p class="subject">{detail.subject}</p>{/if}
        <!-- The model's summary, marked as its own and kept apart from the
             text (design 06). Never mixed into the content. -->
        {#if detail.summary}
          <div class="summary"><span class="ai">{t('item.summary')}</span><p>{detail.summary}</p></div>
        {/if}
        <p>{detail.text}</p>
        {#if detail.tombstoned}<p class="note">{t('item.deleted')}</p>{/if}
        {#if detail.judgements.length}
          <div class="judgements">
            {#each detail.judgements as j}
              <div><b>{j.level}</b> · {j.by}{j.detail ? ' · ' + j.detail : ''} · {j.at}</div>
            {/each}
          </div>
        {/if}
        <div class="actions-line">
          {#if replyable}
            <button type="button" class="choose" disabled={drafting === 'busy' || drafting === 'done'} onclick={draft}>
              {drafting === 'busy' ? t('item.drafting') : drafting === 'done' ? t('item.drafted') : drafting === 'failed' ? draftError : t('item.draft')}
            </button>
          {/if}
        </div>
        <LevelChooser itemId={row.id} current={detail.row.level} ondone={levelChanged} />
      {/if}
    </div>
  {/if}
</li>
