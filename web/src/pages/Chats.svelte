<script lang="ts" module>
  import type { Party } from '../lib/api';
  // The list outlives the page: coming back to Chats shows the list you left,
  // where you left it, and refreshes it quietly behind that.
  let cached: Party[] | null = null;
  let scrolled = 0;
  // Which of the three the list shows (design 06 v0.12): kept while the
  // page is away, like the scroll.
  type Segment = 'talk' | 'agents' | 'work';
  let segment: Segment = 'talk';
</script>

<script lang="ts">
  import { tick } from 'svelte';
  import { get } from '../lib/api';
  import { t } from '../lib/i18n';
  import { initials } from '../lib/format';
  import { go } from '../lib/router';
  import AgentChat from '../components/AgentChat.svelte';
  import QuietList from '../components/QuietList.svelte';
  import Conversation from '../components/Conversation.svelte';
  import Empty from '../components/Empty.svelte';
  import Icon from '../components/Icon.svelte';

  // Design 06 v0.6, "对话": one list of conversations. A party is a person
  // (what passed one to one) or a group or channel (the container is one
  // party; its members are not listed one by one). An installed agent is one
  // more kind of party (design 11), and so is a task, with its comments
  // (v0.12). The list is in three segments: talk, agents, work.
  let { kind = 'person', id = null }: { kind?: 'person' | 'group' | 'agent' | 'quiet' | 'task'; id?: string | null } = $props();
  let showing = $state<Segment>(segment);
  $effect(() => { segment = showing; });
  const segmentOf = (p: Party): Segment => (p.kind === 'agent' ? 'agents' : p.kind === 'task' ? 'work' : 'talk');
  // Opening a party by its address shows the segment it is in.
  $effect(() => {
    const k = kind;
    if (id) showing = k === 'agent' ? 'agents' : k === 'task' ? 'work' : 'talk';
  });
  let parties = $state<Party[] | null>(cached);
  let error = $state('');
  let filter = $state('');
  let list = $state<HTMLElement | null>(null);

  // Once per visit to the page, not per conversation opened: the effect reads
  // nothing reactive, and the page is one instance for every chats route.
  $effect(() => {
    get<Party[]>('/api/chats')
      .then((p) => { parties = p; cached = p; })
      .catch((e) => { if (!parties) { error = e.message; parties = []; } });
  });
  // A party marked as junk leaves the list at once (design 06, "标为垃圾").
  $effect(() => {
    const gone = (e: Event) => {
      const id = (e as CustomEvent<string>).detail;
      if (parties) { parties = parties.filter((p) => p.id !== id); cached = parties; }
    };
    window.addEventListener('genatrix:party-gone', gone);
    return () => window.removeEventListener('genatrix:party-gone', gone);
  });
  // Put the list back where it was: on arrival, and on a phone each time a
  // conversation closes, because a hidden element forgets its scroll.
  $effect(() => {
    const showing = id;
    const el = list;
    if (el) tick().then(() => { if (showing === id) el.scrollTop = scrolled; });
  });
  const inSegment = $derived.by(() => {
    const mine = (parties ?? []).filter((p) => segmentOf(p) === showing);
    // Work: what is still to do first, each part by latest change.
    if (showing === 'work') return [...mine].sort((a, b) => Number(a.status === 'done') - Number(b.status === 'done') || b.last_ms - a.last_ms);
    return mine;
  });
  const shown = $derived.by(() => {
    const q = filter.trim().toLowerCase();
    return q ? inSegment.filter((p) => p.name.toLowerCase().includes(q)) : inSegment;
  });
  const todo = $derived((parties ?? []).filter((p) => p.kind === 'task' && p.status === 'todo').length);
  const hasAgents = $derived((parties ?? []).some((p) => p.kind === 'agent'));
  const firstDone = $derived(shown.findIndex((p) => p.status === 'done'));
  const current = $derived(parties?.find((p) => p.id === id) ?? null);
  const href = (p: Party) =>
    p.kind === 'person' ? `/chats/${p.id}` : p.kind === 'agent' ? `/chats/a/${p.id}`
      : p.kind === 'task' ? `/chats/t/${p.id}` : p.kind === 'quiet' ? '/chats/quiet' : `/chats/g/${p.id}`;
  function pick(s: Segment) {
    showing = s; filter = '';
    if (list) list.scrollTop = 0;
    scrolled = 0;
  }
</script>

<section class="pane chats-pane" class:has-detail={!!id}>
  <div class="chat-list" bind:this={list} onscroll={() => { if (list) scrolled = list.scrollTop; }}>
    <div class="segments" role="tablist">
      <button type="button" role="tab" class="segment" class:is-on={showing === 'talk'} aria-selected={showing === 'talk'} onclick={() => pick('talk')}>{t('chats.talk')}</button>
      <button type="button" role="tab" class="segment" class:is-on={showing === 'agents'} aria-selected={showing === 'agents'} onclick={() => pick('agents')}>{t('chats.agents')}</button>
      <button type="button" role="tab" class="segment" class:is-on={showing === 'work'} aria-selected={showing === 'work'} onclick={() => pick('work')}>
        {t('chats.work')}{#if todo}<span class="segment-count">{todo}</span>{/if}
      </button>
    </div>
    <label class="chat-search">
      <Icon name="search" size={16} />
      <input type="search" bind:value={filter} placeholder={t('chats.search')} autocomplete="off" />
    </label>
    <ol class="parties">
      {#if error}<li><Empty text={error} error /></li>
      {:else if !parties}<li><Empty text={t('loading')} /></li>
      {:else if !inSegment.length}<li><Empty text={t(showing === 'work' ? 'chats.empty.work' : showing === 'agents' ? (hasAgents ? 'chats.empty' : 'chats.empty.agents') : 'chats.empty')} /></li>
      {:else if !shown.length}<li><Empty text={t('chats.nomatch')} /></li>{/if}
      {#each shown as p, i (p.kind + p.id)}
        {#if showing === 'work' && i === firstDone && i > 0}<li class="parties-divider">{t('chats.done')}</li>{/if}
        <li>
          {#if p.kind === 'task'}
          <button type="button" class="party task-party" class:is-on={p.id === id} class:done={p.status === 'done'} onclick={() => go(href(p))}>
            <span class="avatar multi"><Icon name="task" size={18} /></span>
            <span class="party-body">
              <span class="party-top">
                <span class="party-name">{p.name}</span>
                {#if p.due}<span class="party-when" class:due={p.status === 'todo'}>{t('chats.due', { day: p.due.slice(5) })}</span>
                {:else if p.last_at}<span class="party-when">{p.last_at.slice(5)}</span>{/if}
              </span>
              <span class="party-last">
                {#if p.space}<span class="role space-chip">{p.space}</span>{/if}{#if p.from}<b>{p.from_me ? t('chat.you') : p.from}</b>{/if}{#if p.lists?.length}{p.from ? ' · ' : ''}{p.lists.join(' · ')}{/if}
              </span>
              {#if p.last_text && p.messages > 1}<span class="party-last faint">{#if p.last_author}{p.last_mine ? t('chat.you') : p.last_author}: {/if}{p.last_text}</span>{/if}
            </span>
          </button>
          {:else}
          <button type="button" class="party" class:is-on={p.id === id} onclick={() => go(href(p))}>
            <span class="avatar" class:multi={p.kind !== 'person'} class:agent={p.kind === 'agent'}>
              {#if p.kind === 'person'}{initials(p.name)}{:else}<Icon name={p.kind === 'quiet' ? 'mail' : p.kind} size={18} />{/if}
            </span>
            <span class="party-body">
              <span class="party-top">
                <span class="party-name">{p.kind === 'quiet' ? t('quiet.title') : p.name}</span>
                {#if p.last_at}<span class="party-when">{p.last_at.slice(5)}</span>{/if}
              </span>
              <span class="party-last">
                {#if p.kind === 'quiet'}{t('quiet.line', { n: p.messages, who: p.last_text ?? '' })}
                {:else}{#if p.last_author}<b>{p.last_mine ? t('chat.you') : p.last_author}:</b> {/if}{p.last_text || ''}{/if}
              </span>
              {#if p.roles.length}
                <span class="party-roles">{#each p.roles as r}<span class="role">{r}</span>{/each}</span>
              {/if}
            </span>
          </button>
          {/if}
        </li>
      {/each}
    </ol>
  </div>

  {#if id}
    {#key kind + id}
      {#if kind === 'agent'}
        <AgentChat {id} name={current?.name ?? ''} />
      {:else if kind === 'quiet'}
        <QuietList />
      {:else}
        <Conversation kind={kind === 'task' ? 'task' : kind === 'group' ? 'group' : 'person'} {id} name={current?.name ?? ''} />
      {/if}
    {/key}
  {/if}
</section>
