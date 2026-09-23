<script lang="ts">
  import { get, type PersonCard } from '../lib/api';
  import { t } from '../lib/i18n';
  import { initials } from '../lib/format';
  import { go } from '../lib/router';
  import Conversation from '../components/Conversation.svelte';
  import Empty from '../components/Empty.svelte';
  import Icon from '../components/Icon.svelte';

  // Design 06 v0.5, "对话": one list of the parties you talk to, and one
  // conversation per party. Today every party is a person; an agent will be
  // one more kind of party in the same list.
  let { id = null }: { id?: string | null } = $props();
  let cards = $state<PersonCard[] | null>(null);
  let error = $state('');
  let filter = $state('');

  $effect(() => {
    get<PersonCard[]>('/api/people?limit=200')
      .then((c) => { cards = c; })
      .catch((e) => { error = e.message; cards = []; });
  });
  const shown = $derived.by(() => {
    const q = filter.trim().toLowerCase();
    if (!cards) return [];
    return q ? cards.filter((c) => c.name.toLowerCase().includes(q) || c.handles.some((h) => h.value.toLowerCase().includes(q))) : cards;
  });
  const current = $derived(cards?.find((c) => c.id === id) ?? null);
</script>

<section class="pane chats-pane" class:has-detail={!!id}>
  <div class="chat-list">
    <label class="chat-search">
      <Icon name="search" size={16} />
      <input type="search" bind:value={filter} placeholder={t('chats.search')} autocomplete="off" />
    </label>
    <ol class="parties">
      {#if error}<li><Empty text={error} error /></li>
      {:else if !cards}<li><Empty text={t('loading')} /></li>
      {:else if !cards.length}<li><Empty text={t('chats.empty')} /></li>
      {:else if !shown.length}<li><Empty text={t('chats.nomatch')} /></li>{/if}
      {#each shown as card (card.id)}
        <li>
          <button type="button" class="party" class:is-on={card.id === id} onclick={() => go(`/chats/${card.id}`)}>
            <span class="avatar">{initials(card.name)}</span>
            <span class="party-body">
              <span class="party-top">
                <span class="party-name">{card.name}</span>
                {#if card.last_at}<span class="party-when">{card.last_at.slice(5)}</span>{/if}
              </span>
              <span class="party-last">{card.last_text || ''}</span>
              {#if card.roles.length}
                <span class="party-roles">{#each card.roles as r}<span class="role">{r}</span>{/each}</span>
              {/if}
            </span>
          </button>
        </li>
      {/each}
    </ol>
  </div>

  {#if id}
    {#key id}
      <Conversation {id} card={current} />
    {/key}
  {/if}
</section>
