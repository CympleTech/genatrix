<script lang="ts">
  import { t } from '../lib/i18n';

  // Design 11 v0.2, design 15: an agent's view, drawn here. The agent sent
  // data of a fixed shape; how it looks is the interface's.
  type Cell = { text: string } | { number: number } | { money: { cents: number; currency: string } } | { date: string };
  interface View {
    title: string; note: string;
    figures: { label: string; value: Cell }[];
    columns: { key: string; label: string }[];
    rows: Cell[][];
    series: { x: string; y: number }[];
  }
  let { view, at }: { view: View; at: string } = $props();

  function cell(c: Cell): string {
    if ('text' in c) return c.text;
    if ('number' in c) return c.number.toLocaleString();
    if ('money' in c) return (c.money.cents / 100).toLocaleString(undefined, { style: 'currency', currency: c.money.currency || 'NZD' });
    return c.date;
  }
  const top = $derived(Math.max(1e-9, ...view.series.map((p) => Math.abs(p.y))));
</script>

<div class="agent-view">
  <h4 class="card-label">{view.title}</h4>
  {#if view.note}<p class="note">{view.note}</p>{/if}
  {#if view.figures.length}
    <div class="view-figures">
      {#each view.figures as f}<div class="view-figure"><span class="view-figure-n">{cell(f.value)}</span><span class="faint">{f.label}</span></div>{/each}
    </div>
  {/if}
  {#if view.columns.length && view.rows.length}
    <table class="view-table">
      <thead><tr>{#each view.columns as c (c.key)}<th>{c.label}</th>{/each}</tr></thead>
      <tbody>{#each view.rows as r}<tr>{#each r as c}<td class:num={'number' in c || 'money' in c}>{cell(c)}</td>{/each}</tr>{/each}</tbody>
    </table>
  {/if}
  {#if view.series.length}
    <ol class="view-series">
      {#each view.series as p}<li><span class="faint">{p.x}</span><span class="view-bar"><span style="width:{Math.max(2, (Math.abs(p.y) / top) * 100)}%"></span></span><span>{p.y.toLocaleString()}</span></li>{/each}
    </ol>
  {/if}
  <p class="field-note">{t('agents.view.at', { at: new Date(at).toLocaleString() })}</p>
</div>
