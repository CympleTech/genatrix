<script lang="ts">
  import { post, type AskReply } from '../lib/api';
  import { t } from '../lib/i18n';
  import { status, refreshPending } from '../lib/status';
  import SourceChip from '../components/SourceChip.svelte';
  import ActionCard from '../components/ActionCard.svelte';
  import RunRecord from '../components/RunRecord.svelte';
  import Icon from '../components/Icon.svelte';

  interface Turn { question: string; reply?: AskReply; error?: string; open?: boolean }
  // This session only; gone with the page (design 03: a conversation's
  // context is not memory).
  let turns = $state<Turn[]>([]);
  let question = $state('');
  let busy = $state(false);
  let input = $state<HTMLInputElement | null>(null);
  const ready = $derived($status?.model?.state === 'ready');

  async function ask(e: Event) {
    e.preventDefault();
    const q = question.trim();
    if (!q || busy) return;
    question = '';
    busy = true;
    const turn: Turn = { question: q };
    turns.push(turn);
    const history = turns.filter((x) => x.reply).slice(-6).map((x) => ({ user: x.question, answer: x.reply!.answer }));
    try {
      const reply = await post<AskReply>('/api/ask', { question: q, history });
      turn.reply = reply;
      if (reply.actions.length) refreshPending();
    } catch (err: any) { turn.error = err.message; }
    finally { busy = false; queueMicrotask(() => { input?.focus(); window.scrollTo({ top: document.body.scrollHeight }); }); }
  }
</script>

<!-- Design 06: not the main entrance, a complement. Under every answer the
     items it cites; the tools it used as one folded line that opens into the
     run record; any action it proposed as the same card the Approvals page
     shows; and where it was answered. No avatar, no name, no pleasantries. -->
<section class="pane ask-pane">
  <p class="note">{t('ask.note')}</p>
  <ol class="turns">
    {#each turns as turn, i (i)}
      <li class="turn question">{turn.question}</li>
      <li class="turn answer" class:faint={!turn.reply && !turn.error}>
        {#if turn.error}<span class="error">{turn.error}</span>
        {:else if !turn.reply}{t('ask.looking')}
        {:else}
          <p class="answer-text" class:stopped={turn.reply.stopped}>{turn.reply.answer}</p>
          {#if turn.reply.cited.length}
            <div class="sources">{#each turn.reply.cited as s}<SourceChip src={s} />{/each}</div>
          {/if}
          <div class="meta">
            {#if turn.reply.steps.length}
              <button type="button" class="fold" onclick={() => (turn.open = !turn.open)}>{turn.reply.steps.join(' · ')}</button>
            {:else}<span class="faint">{t('ask.nolookup')}</span>{/if}
            <span class="faint">{turn.reply.answered}</span>
          </div>
          {#if turn.open}<RunRecord runId={turn.reply.run_id} />{/if}
          {#if turn.reply.actions.length}
            <ol class="rows cards mt-3">{#each turn.reply.actions as a (a.id)}<ActionCard action={a} />{/each}</ol>
          {/if}
        {/if}
      </li>
    {/each}
  </ol>
  <form class="ask" onsubmit={ask}>
    <input bind:this={input} bind:value={question} placeholder={ready ? t('ask.placeholder') : t('ask.notready')} disabled={busy || !ready} autocomplete="off" />
    <button type="submit" class="choose primary" disabled={busy || !ready || !question.trim()}>{t('ask.send')}</button>
  </form>
</section>
