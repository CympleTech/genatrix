<script lang="ts">
  import { tick } from 'svelte';
  import { get, post, type AgentDetail, type Turn } from '../lib/api';
  import { t } from '../lib/i18n';
  import { bytes } from '../lib/format';
  import { go } from '../lib/router';
  import { refreshPending } from '../lib/status';
  import ActionCard from './ActionCard.svelte';
  import Icon from './Icon.svelte';

  // Design 11, "身份与记录": an agent is a party in Chats. Above, what it is
  // allowed and how it has done; below, the conversation: what you said on
  // the right, what it answered on the left, what woke it in between, and
  // what it proposed as approval cards in the stream.
  let { id, name: given = '' }: { id: string; name?: string } = $props();

  let detail = $state<AgentDetail | null>(null);
  let turns = $state<Turn[]>([]);
  let error = $state('');
  let text = $state('');
  let sending = $state(false);
  let expanded = $state(false);
  let panel = $state<HTMLElement | null>(null);
  let stream = $state<HTMLElement | null>(null);

  const name = $derived(given || detail?.card.name || '');

  function fit() {
    if (!panel) return;
    const bar = document.querySelector('.bar') as HTMLElement | null;
    const barH = bar && getComputedStyle(bar).display !== 'none' ? bar.offsetHeight : 0;
    panel.style.height = `${Math.max(320, window.innerHeight - panel.getBoundingClientRect().top - barH - 12)}px`;
  }
  $effect(() => {
    window.scrollTo(0, 0);
    fit();
    window.addEventListener('resize', fit);
    return () => window.removeEventListener('resize', fit);
  });
  async function toBottom() {
    await tick();
    if (stream) stream.scrollTop = stream.scrollHeight;
  }
  async function load() {
    try {
      detail = await get<AgentDetail>(`/api/agent/${id}`);
      turns = detail.turns;
      error = '';
      await toBottom();
    } catch (e: any) { error = e.message; }
  }
  $effect(() => { load(); });

  async function send(e: Event) {
    e.preventDefault();
    const said = text.trim();
    if (!said || sending) return;
    sending = true; error = '';
    try {
      const turn = await post<Turn>(`/api/agent/${id}/message`, { text: said });
      turns = [...turns, turn];
      text = '';
      if (turn.actions.length) refreshPending();
      await toBottom();
    } catch (err: any) { error = err.message; }
    sending = false;
  }
  async function toggleState() {
    if (!detail) return;
    try {
      await post(`/api/agent/${id}/${detail.card.state === 'active' ? 'pause' : 'resume'}`, {});
      await load();
    } catch (e: any) { error = e.message; }
  }
</script>

<div class="conversation" bind:this={panel}>
  <header class="conv-head">
    <button type="button" class="back" onclick={() => go('/chats')} aria-label={t('people.back')}><Icon name="back" size={18} /></button>
    <span class="avatar multi agent"><Icon name="agent" size={18} /></span>
    <div class="conv-title">
      <span class="conv-name">{name}</span>
      <span class="conv-sub">
        {#if detail}
          <span class="role">{t('chat.agent')}</span>
          <span class="faint">{detail.card.state === 'paused' ? t('agents.paused') : t('agents.active')} · {t(`agents.risk.${detail.card.risk}`)}</span>
        {/if}
      </span>
    </div>
    <button type="button" class="back conv-toggle" onclick={() => (expanded = !expanded)} aria-expanded={expanded}>
      {expanded ? t('chat.hide') : t('chat.details')}<span class="chev" class:up={expanded}><Icon name="chevron" size={14} /></span>
    </button>
  </header>

  {#if detail}
    <div class="conv-summary">
      <span><b>{detail.card.runs}</b> {t('agents.runsShort')}</span>
      <span><b>{detail.card.approved}</b> {t('agents.approved')}</span>
      <span><b>{detail.card.declined}</b> {t('agents.declined')}</span>
      {#if detail.card.pending}<span><b>{detail.card.pending}</b> {t('agents.pending')}</span>{/if}
      <span><b>{bytes(detail.card.space_bytes)}</b> {t('agents.space')} · {t(`level.${detail.card.level}`)}</span>
    </div>
  {/if}

  {#if expanded && detail}
    <div class="conv-details">
      <p class="preview-purpose">{detail.card.purpose}</p>
      <dl class="card-fields grants">
        {#each detail.lines as l}<dt>{t(`agents.grant.${l.label}`)}</dt><dd>{l.text}</dd>{/each}
      </dl>
      <p class="note">{t('agents.by', { author: detail.card.author })} · {t('agents.version', { v: detail.card.version })}</p>
      <div class="chooser">
        <button type="button" class="choose" class:lowers={detail.card.state === 'active'} onclick={toggleState}>
          {detail.card.state === 'active' ? t('agents.pause') : t('agents.resume')}
        </button>
        <a class="choose" href={`/api/agent/${id}/export`} download>{t('agents.export')}</a>
      </div>
    </div>
  {/if}

  <div class="stream" bind:this={stream}>
    {#if error}<p class="empty error">{error}</p>{/if}
    {#if !detail && !error}<p class="empty">{t('loading')}</p>
    {:else if detail && !turns.length}<p class="empty">{t('agents.chat.empty')}</p>{/if}
    {#each turns as turn, i (i)}
      {#if turn.you}
        <div class="msg mine"><div class="bubble"><p class="bubble-text">{turn.you}</p>
          <div class="bubble-foot"><span class="msg-time">{turn.at}</span></div></div></div>
      {:else if turn.woke_by}
        <div class="day"><span>{turn.at} · {turn.woke_by}</span></div>
      {/if}
      {#if turn.answer}
        <div class="msg"><div class="bubble"><p class="bubble-text">{turn.answer}</p>
          <div class="bubble-foot"><span class="msg-time">{t('agents.read', { n: turn.reads })}</span></div></div></div>
      {:else if turn.outcome !== 'ok'}
        <div class="msg"><div class="bubble failed"><p class="bubble-text">{t(`agents.outcome.${turn.outcome}`)}{turn.detail ? ': ' + turn.detail : ''}</p></div></div>
      {/if}
      {#if turn.actions.length}
        <ol class="rows cards stream-drafts">{#each turn.actions as a (a.id)}<ActionCard action={a} />{/each}</ol>
      {/if}
    {/each}
  </div>

  <form class="composer" onsubmit={send}>
    <input class="composer-input" bind:value={text} placeholder={t('agents.say')} disabled={sending || detail?.card.state === 'paused'} />
    <button type="submit" class="choose primary" disabled={sending || !text.trim() || detail?.card.state === 'paused'}>
      <Icon name="send" size={16} />{sending ? t('agents.working') : t('agents.send')}
    </button>
  </form>
</div>
