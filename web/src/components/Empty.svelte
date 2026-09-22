<script lang="ts">
  let { text, error = false }: { text: string; error?: boolean } = $props();
</script>

<p class="empty" class:error>{text}</p>
