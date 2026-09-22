<script lang="ts">
  import { get, type SourceRef, type Detail } from '../lib/api';
  import { connectorName } from '../lib/format';
  import { t } from '../lib/i18n';

  let { src }: { src: SourceRef } = $props();
  let open = $state(false);
  let detail = $state<Detail | null>(null);
  let error = $state('');

  async function toggle() {
    open = !open;
    if (open && !detail) {
      try { detail = await get<Detail>(`/api/item/${src.id}`); } catch (e: any) { error = e.message; }
    }
  }
</script>

<!-- A source: who, when, where; click to read the item in place. -->
<span class="source-wrap">
  <button type="button" class="source" onclick={toggle}>{src.who} · {connectorName(src.connector)} · {src.at}</button>
  {#if open}
    <div class="detail">
      {#if error}<p class="error">{error}</p>
      {:else if !detail}<p>{t('loading')}</p>
      {:else}
        {#if detail.subject}<p class="subject">{detail.subject}</p>{/if}
        <p>{detail.text}</p>
      {/if}
    </div>
  {/if}
</span>
