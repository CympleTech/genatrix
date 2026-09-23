<script lang="ts">
  import { get, type Party } from '../lib/api';
  import { t } from '../lib/i18n';
  import { initials } from '../lib/format';
  import { go } from '../lib/router';
  import Conversation from '../components/Conversation.svelte';
  import Empty from '../components/Empty.svelte';
  import Icon from '../components/Icon.svelte';

  // Design 06 v0.6, "对话": one list of conversations. A party is a person
  // (what passed one to one) or a group or channel (the container is one
  // party; its members are not listed one by one). An agent will be one more
  // kind of party in the same list.
  let { kind = 'person', id = null }: { kind?: 'person' | 'group'; id?: string | null } = $props();
  let parties = $state<Party[] | null>(null);
  let error = $state('');
  let filter = $state('');

  $effect(() => {
    get<Party[]>('/api/chats')
      .then((p) => { parties = p; })
      .catch((e) => { error = e.message; parties = []; });
  });
  const shown = $derived.by(() => {
    const q = filter.trim().toLowerCase();
    if (!parties) return [];
    return q ? parties.filter((p) => p.name.toLowerCase().includes(q)) : parties;
  });
  const current = $derived(parties?.find((p) => p.id === id) ?? null);
  const href = (p: Party) => (p.kind === 'person' ? `/chats/${p.id}` : `/chats/g/${p.id}`);
</script>

<section class="pane chats-pane" class:has-detail={!!id}>
  <div class="chat-list">
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
            <span class="avatar" class:multi={p.kind !== 'person'}>
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
      <Conversation {kind} {id} name={current?.name ?? ''} />
    {/key}
  {/if}
</section>
