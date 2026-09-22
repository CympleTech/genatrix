<script lang="ts">
  import { post, type Detail } from '../lib/api';
  import { LEVELS, levelWord } from '../lib/format';
  import { t } from '../lib/i18n';

  let { itemId, current, agreeWith = null, ondone }: {
    itemId: string; current: string; agreeWith?: string | null; ondone: (d: Detail) => void;
  } = $props();
  let busy = $state(false);
  let error = $state('');

  // Design 06: three options. Raising takes effect on the click. Lowering
  // says what it means in the option itself, and the click on that line is
  // the confirmation; no modal, no second "OK".
  function label(level: string): string {
    const rank = LEVELS.indexOf(level as any) - LEVELS.indexOf(current as any);
    if (level === agreeWith) return t('level.agree', { level: levelWord(level) });
    if (rank > 0) return t('level.raise', { level: levelWord(level) });
    if (rank < 0) return t('level.lower', { level: levelWord(level) });
    return t('level.keep', { level: levelWord(level) });
  }
  function lowers(level: string): boolean {
    return LEVELS.indexOf(level as any) < LEVELS.indexOf(current as any);
  }
  async function choose(level: string) {
    busy = true;
    try { ondone(await post<Detail>(`/api/item/${itemId}/level`, { level })); }
    catch (e: any) { error = e.message; busy = false; }
  }
</script>

<div class="chooser">
  {#each LEVELS as level}
    <button type="button" class="choose level-{level}" class:lowers={lowers(level)} disabled={busy} onclick={() => choose(level)}>{label(level)}</button>
  {/each}
  {#if error}<span class="error">{error}</span>{/if}
</div>
