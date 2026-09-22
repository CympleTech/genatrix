<script lang="ts">
  import { levelWord } from '../lib/format';
  let { level, reason = '' }: { level: string; reason?: string } = $props();
</script>

<!-- A level is a dot and a word. Colour alone would leave the difference
     invisible to some readers; the word carries the meaning (design 06). -->
<span class="level level-{level}" title={reason}>{levelWord(level)}</span>
