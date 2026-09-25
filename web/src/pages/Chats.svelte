<script lang="ts" module>
  import type { Party } from '../lib/api';
  // The list outlives the page: coming back to Chats shows the list you left,
  // where you left it, and refreshes it quietly behind that.
  let cached: Party[] | null = null;
  let scrolled = 0;
</script>

<script lang="ts">
  import { tick } from 'svelte';
  import { get } from '../lib/api';
  import { t } from '../lib/i18n';
  import { initials } from '../lib/format';
  import { go } from '../lib/router';
  import AgentChat from '../components/AgentChat.svelte';
  import Conversation from '../components/Conversation.svelte';
  import Empty from '../components/Empty.svelte';
  import Icon from '../components/Icon.svelte';

  // Design 06 v0.6, "对话": one list of conversations. A party is a person
  // (what passed one to one) or a group or channel (the container is one
  // party; its members are not listed one by one). An installed agent is one
  // more kind of party in the same list (design 11).
  let { kind = 'person', id = null }: { kind?: 'person' | 'group' | 'agent'; id?: string | null } = $props();
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
  // Put the list back where it was: on arrival, and on a phone each time a
  // conversation closes, because a hidden element forgets its scroll.
  $effect(() => {
    const showing = id;
    const el = list;
    if (el) tick().then(() => { if (showing === id) el.scrollTop = scrolled; });
  });
  const shown = $derived.by(() => {
    const q = filter.trim().toLowerCase();
    if (!parties) return [];
    return q ? parties.filter((p) => p.name.toLowerCase().includes(q)) : parties;
  });
  const current = $derived(parties?.find((p) => p.id === id) ?? null);
  const href = (p: Party) =>
    p.kind === 'person' ? `/chats/${p.id}` : p.kind === 'agent' ? `/chats/a/${p.id}` : `/chats/g/${p.id}`;
</script>

<section class="pane chats-pane" class:has-detail={!!id}>
  <div class="chat-list" bind:this={list} onscroll={() => { if (list) scrolled = list.scrollTop; }}>
    <label class="chat-search">
      <Icon name="search" size={16} />
      <input type="search" bind:value={filter} placeholder={t('chats.search')} autocomplete="off" />
    </label>
    <ol class="parties">
      {#if error}<li><Empty text={error} error /></li>
      {:else if !parties}<li><Empty text={t('loading')} /></li>
      {:else if !parties.length}<li><Empty text={t('chats.empty')} /></li>
      {:else if !shown.length}<li><Empty text={t('chats.nomatch')} /></li>{/if}
      {#each shown as p (p.kind + p.id)}
        <li>
          <button type="button" class="party" class:is-on={p.id === id} onclick={() => go(href(p))}>
            <span class="avatar" class:multi={p.kind !== 'person'} class:agent={p.kind === 'agent'}>
              {#if p.kind === 'person'}{initials(p.name)}{:else}<Icon name={p.kind} size={18} />{/if}
            </span>
            <span class="party-body">
              <span class="party-top">
                <span class="party-name">{p.name}</span>
                {#if p.last_at}<span class="party-when">{p.last_at.slice(5)}</span>{/if}
              </span>
              <span class="party-last">
                {#if p.last_author}<b>{p.last_author}:</b> {/if}{p.last_text || ''}
              </span>
              {#if p.roles.length}
                <span class="party-roles">{#each p.roles as r}<span class="role">{r}</span>{/each}</span>
              {/if}
            </span>
          </button>
        </li>
      {/each}
    </ol>
  </div>

  {#if id}
    {#key kind + id}
      {#if kind === 'agent'}
        <AgentChat {id} name={current?.name ?? ''} />
      {:else}
        <Conversation {kind} {id} name={current?.name ?? ''} />
      {/if}
    {/key}
  {/if}
</section>
