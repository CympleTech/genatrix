<script lang="ts">
  import { tick } from 'svelte';
  import {
    get, post,
    type Action, type ChatMessage, type ChatPage, type Detail, type GroupDetail, type PersonDetail,
  } from '../lib/api';
  import { t } from '../lib/i18n';
  import { connectorName, hours, initials } from '../lib/format';
  import { go } from '../lib/router';
  import { refreshPending } from '../lib/status';
  import ActionCard from './ActionCard.svelte';
  import CommitmentRow from './CommitmentRow.svelte';
  import LevelMark from './LevelMark.svelte';
  import LevelChooser from './LevelChooser.svelte';
  import Icon from './Icon.svelte';

  // Design 06 v0.6, "对话". Three parts, top to bottom: the facts about this
  // party (arithmetic and what you wrote, never the model's opinion), the
  // conversation (others on the left, yours on the right, newest at the
  // bottom), and a reply that goes through you: a draft becomes an approval
  // card in the stream, and nothing is sent from here.
  //
  // A party is a person, whose conversation is what passed one to one, or a
  // group or channel, which is one party however many people are in it.
  let { kind, id, name: given = '' }: { kind: 'person' | 'group'; id: string; name?: string } = $props();

  let person = $state<PersonDetail | null>(null);
  let group = $state<GroupDetail | null>(null);
  let messages = $state<ChatMessage[]>([]);
  let earlier = $state(false);
  let loading = $state(true);
  let loadingEarlier = $state(false);
  let error = $state('');
  let expanded = $state(typeof window !== 'undefined' && window.innerWidth >= 900);
  let opened = $state<Record<string, Detail | 'loading'>>({});
  let drafts = $state<Action[]>([]);
  let drafting = $state(false);
  let draftError = $state('');
  let roles = $state<string[]>([]);
  let notes = $state('');
  let newRole = $state('');

  let panel = $state<HTMLElement | null>(null);
  let stream = $state<HTMLElement | null>(null);

  const base = $derived(kind === 'person' ? `/api/person/${id}` : `/api/group/${id}`);
  const name = $derived(given || person?.card.name || group?.name || '');
  const open = $derived(person?.commitments.filter((c) => c.status === 'open' || c.status === 'overdue') ?? []);
  // In a person's conversation every message is one to one; in a group the
  // reply goes to the group, which is what you are there to do.
  const target = $derived(
    [...messages].reverse().find((m) => !m.mine && (kind === 'group' || m.place === 'direct' || m.place === 'mail')) ?? null,
  );

  // The panel takes the height the window has left, so the facts stay put and
  // only the conversation scrolls, the way a conversation is read.
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

  $effect(() => {
    if (kind === 'person') {
      get<PersonDetail>(base)
        .then((d) => { person = d; roles = [...d.card.roles]; notes = d.notes || ''; })
        .catch((e) => { error = e.message; });
    } else {
      get<GroupDetail>(base).then((d) => { group = d; }).catch((e) => { error = e.message; });
    }
    get<ChatPage>(`${base}/chat?limit=40`)
      .then(async (p) => { messages = p.messages; earlier = p.earlier; loading = false; await toBottom(); })
      .catch((e) => { error = e.message; loading = false; });
  });

  async function loadEarlier() {
    if (!messages.length || loadingEarlier) return;
    loadingEarlier = true;
    try {
      const p = await get<ChatPage>(`${base}/chat?limit=40&before=${messages[0].ms}`);
      const before = stream?.scrollHeight ?? 0;
      messages = [...p.messages, ...messages];
      earlier = p.earlier;
      await tick();
      // Keep the message you were reading where it was.
      if (stream) stream.scrollTop += stream.scrollHeight - before;
    } catch (e: any) { error = e.message; }
    loadingEarlier = false;
  }

  async function toggle(m: ChatMessage) {
    if (opened[m.id]) { const { [m.id]: _, ...rest } = opened; opened = rest; return; }
    opened = { ...opened, [m.id]: 'loading' };
    try { opened = { ...opened, [m.id]: await get<Detail>(`/api/item/${m.id}`) }; }
    catch { const { [m.id]: _, ...rest } = opened; opened = rest; }
  }

  async function draft() {
    if (!target) return;
    drafting = true; draftError = '';
    try {
      drafts = [...drafts, await post<Action>(`/api/item/${target.id}/draft`, {})];
      refreshPending();
      await toBottom();
    } catch (e: any) { draftError = e.message; }
    drafting = false;
  }

  async function save() {
    try { await post(`/api/person/${id}/relationship`, { roles, notes }); } catch (e: any) { error = e.message; }
  }
  function addRole() { const r = newRole.trim(); if (!r) return; roles = [...roles, r]; newRole = ''; save(); }
  function removeRole(r: string) { roles = roles.filter((x) => x !== r); save(); }

  function dayLabel(day: string): string {
    const d = new Date(day + 'T00:00:00');
    const today = new Date(); today.setHours(0, 0, 0, 0);
    const diff = Math.round((today.getTime() - d.getTime()) / 86400000);
    if (diff === 0) return t('chat.today');
    if (diff === 1) return t('chat.yesterday');
    return day;
  }
  function long(m: ChatMessage): boolean {
    return m.folded || m.text.length > 420 || m.text.split('\n').length > 9;
  }
  // In a group, say who is talking whenever the voice changes.
  function showAuthor(i: number): boolean {
    const m = messages[i];
    if (m.mine || !m.author) return false;
    const prev = messages[i - 1];
    return !prev || prev.mine || prev.author !== m.author || prev.day !== m.day;
  }
  function bars(months: number[]) {
    const max = Math.max(1, ...months);
    return months.map((n) => Math.max(2, Math.round((n / max) * 28)));
  }
</script>

<div class="conversation" bind:this={panel}>
  <header class="conv-head">
    <button type="button" class="back" onclick={() => go('/chats')} aria-label={t('people.back')}><Icon name="back" size={18} /></button>
    <span class="avatar" class:multi={kind === 'group'}>
      {#if kind === 'group'}<Icon name={group?.kind ?? 'group'} size={18} />{:else}{initials(name)}{/if}
    </span>
    <div class="conv-title">
      <span class="conv-name">{name}</span>
      <span class="conv-sub">
        {#if kind === 'person'}
          {#each roles as r}<span class="role">{r}</span>{/each}
          {#if person}<span class="faint">{person.stats.connectors.map(([c]) => connectorName(c)).join(' · ')}</span>{/if}
        {:else if group}
          <span class="faint">{t(group.kind === 'channel' ? 'chat.channel' : 'chat.group')} · {t('chat.voices', { n: group.voices })}</span>
        {/if}
      </span>
    </div>
    <button type="button" class="back conv-toggle" onclick={() => (expanded = !expanded)} aria-expanded={expanded}>
      {expanded ? t('chat.hide') : t('chat.details')}<span class="chev" class:up={expanded}><Icon name="chevron" size={14} /></span>
    </button>
  </header>

  {#if person}
    <div class="conv-summary">
      <span><b>↓ {person.stats.from_them}</b> {t('people.fromthem')}</span>
      <span><b>↑ {person.stats.to_them}</b> {t('people.fromyou')}</span>
      {#if person.stats.reply_hours != null}<span><b>{hours(person.stats.reply_hours)}</b> {t('people.replytime')}</span>{/if}
      {#if person.stats.first_at}<span>{t('people.since')} <b>{person.stats.first_at}</b></span>{/if}
      <button type="button" class="promise-chip" class:has={open.length > 0} onclick={() => (expanded = true)}>
        {open.length === 0 ? t('chat.nopromises') : open.length === 1 ? t('chat.promise') : t('chat.promises', { n: open.length })}
      </button>
    </div>
  {:else if group}
    <div class="conv-summary">
      <span><b>{group.messages}</b> {t('chat.messages')}</span>
      <span><b>{group.mine}</b> {t('chat.yours')}</span>
      <span><b>{group.voices}</b> {t('chat.voicesShort')}</span>
      {#if group.first_at}<span>{t('people.since')} <b>{group.first_at}</b></span>{/if}
    </div>
  {/if}

  {#if expanded && person}
    <div class="conv-details">
      {#if open.length}
        <h3 class="group-title">{t('people.promises', { n: open.length })}</h3>
        <ol class="rows">{#each open as c (c.id)}<CommitmentRow {c} />{/each}</ol>
      {/if}
      <div class="details-grid">
        <div>
          <h3 class="group-title">{t('people.months')}</h3>
          <div class="months">
            {#each bars(person.stats.months) as h, i}
              <span class="month" class:now={i === person.stats.months.length - 1} style="height:{h}px" title={String(person.stats.months[i])}></span>
            {/each}
          </div>
          <h3 class="group-title">{t('chat.roles')}</h3>
          <div class="roles">
            {#each roles as r}<button type="button" class="role removable" onclick={() => removeRole(r)}>{r}</button>{/each}
            <input class="role-input" bind:value={newRole} placeholder={t('people.addrole')} onkeydown={(e) => { if (e.key === 'Enter') addRole(); }} />
          </div>
          <h3 class="group-title">{t('chat.handles')}</h3>
          <div class="handles">{#each person.card.handles as h}<span class="handle" class:inferred={h.inferred}>{h.value}</span>{/each}</div>
        </div>
        <div>
          <h3 class="group-title">{t('people.notes')}</h3>
          <textarea class="notes" bind:value={notes} onblur={save} placeholder={t('people.notes.hint')}></textarea>
        </div>
      </div>
    </div>
  {:else if expanded && group}
    <div class="conv-details">
      <h3 class="group-title">{t('chat.speakers')}</h3>
      <ol class="speakers">
        {#each group.speakers as sp}
          <li class="speaker">
            <span class="avatar small">{initials(sp.name)}</span>
            <span class="speaker-name">{sp.me ? t('chat.you') : sp.name}</span>
            <span class="speaker-bar"><span style="width:{Math.max(6, (sp.count / Math.max(1, group.speakers[0]?.count ?? 1)) * 100)}%"></span></span>
            <span class="faint">{sp.count}</span>
          </li>
        {/each}
      </ol>
    </div>
  {/if}

  <div class="stream" bind:this={stream}>
    {#if error}<p class="empty error">{error}</p>{/if}
    {#if loading}<p class="empty">{t('loading')}</p>
    {:else if !messages.length}<p class="empty">{t('chat.empty')}</p>
    {:else}
      <div class="stream-top">
        {#if earlier}
          <button type="button" class="back" disabled={loadingEarlier} onclick={loadEarlier}>{loadingEarlier ? t('loading') : t('chat.loadEarlier')}</button>
        {:else}<span class="faint">{t('chat.beginning')}</span>{/if}
      </div>
      {#each messages as m, i (m.id)}
        {#if i === 0 || messages[i - 1].day !== m.day}<div class="day"><span>{dayLabel(m.day)}</span></div>{/if}
        {@const tight = i > 0 && messages[i - 1].mine === m.mine && messages[i - 1].day === m.day && !showAuthor(i)}
        {@const d = opened[m.id]}
        <div class="msg" class:mine={m.mine} class:tight>
          {#if showAuthor(i)}<span class="msg-author">{m.author}</span>{/if}
          <div class="bubble" class:mail={m.place === 'mail'}>
            {#if m.subject}<p class="bubble-subject">{m.subject}</p>{/if}
            {#if d && d !== 'loading'}
              {#if d.summary}<div class="summary"><span class="ai">{t('item.summary')}</span><p>{d.summary}</p></div>{/if}
              <p class="bubble-text">{d.text}</p>
            {:else}
              <p class="bubble-text" class:clamped={long(m)}>{m.text}</p>
            {/if}
            <div class="bubble-foot">
              <span class="msg-time">{m.time}{m.place === 'mail' ? ' · ' + t('chat.bymail') : ''}</span>
              <button type="button" class="bubble-more" onclick={() => toggle(m)}>
                {d === 'loading' ? t('loading') : d ? t('chat.less') : long(m) ? t('chat.full') : '···'}
              </button>
            </div>
            {#if d && d !== 'loading'}
              <div class="bubble-item">
                <LevelMark level={d.row.level} reason={d.row.level_reason} />
                {#each d.judgements as j}<span class="faint">{j.by}{j.detail ? ' · ' + j.detail : ''}</span>{/each}
                <LevelChooser itemId={m.id} current={d.row.level} ondone={(nd) => (opened = { ...opened, [m.id]: nd })} />
              </div>
            {/if}
          </div>
        </div>
      {/each}
      {#if drafts.length}
        <ol class="rows cards stream-drafts">{#each drafts as a (a.id)}<ActionCard action={a} />{/each}</ol>
      {/if}
    {/if}
  </div>

  <div class="composer">
    <button type="button" class="choose primary" disabled={!target || drafting} onclick={draft}>
      <Icon name="ask" size={16} />{drafting ? t('chat.drafting') : t('chat.draft')}
    </button>
    <span class="composer-note" class:error={!!draftError}>
      {draftError || (!target ? t('chat.nodraftable') : kind === 'group' ? t('chat.draftnote.group') : t('chat.draftnote'))}
    </span>
  </div>
</div>
