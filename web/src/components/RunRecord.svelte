<script lang="ts">
  import { get, type RunStep } from '../lib/api';
  import { t } from '../lib/i18n';

  let { runId }: { runId: string } = $props();
  let steps = $state<RunStep[] | null>(null);
  let error = $state('');

  $effect(() => {
    get<RunStep[]>(`/api/run/${runId}`).then((s) => { steps = s; }).catch((e) => { error = e.message; });
  });

  function line(s: RunStep): string {
    const b = s.body;
    if (s.kind === 'run') return t('run.budget', { task: b.task, n: b.max_steps });
    if (s.kind === 'run_end') return b.end === 'done' ? t('run.done', { n: b.steps }) : t('run.stopped', { n: b.steps, reason: b.reason });
    if (b.step === 'model') return `model · ${b.purpose} · attempt ${b.attempt} · ${b.result.result}${b.result.reason ? ': ' + b.result.reason : ''} · egress ${b.egress}`;
    if (b.step === 'tool') return `tool · ${b.tool} ${JSON.stringify(b.arguments)} → ${b.items.length} item(s)`;
    if (b.step === 'note') return `note · ${b.name}: ${b.detail}`;
    return JSON.stringify(b);
  }
</script>

<div class="detail record">
  {#if error}<p class="error">{error}</p>
  {:else if !steps}<p>{t('loading')}</p>
  {:else}
    {#each steps as s}<p class="step"><time>{s.at}</time> {line(s)}</p>{/each}
  {/if}
</div>
