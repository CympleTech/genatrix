<script lang="ts">
  import { get, post, type PersonCard, type PersonDetail } from '../lib/api';
  import { t } from '../lib/i18n';
  import { connectorName, hours, initials } from '../lib/format';
  import { go } from '../lib/router';
  import ItemRow from '../components/ItemRow.svelte';
  import CommitmentRow from '../components/CommitmentRow.svelte';
  import Empty from '../components/Empty.svelte';
  import Icon from '../components/Icon.svelte';

  let { id = null }: { id?: string | null } = $props();
  let cards = $state<PersonCard[] | null>(null);
  let detail = $state<PersonDetail | null>(null);
  let error = $state('');
  let roles = $state<string[]>([]);
  let notes = $state('');
  let newRole = $state('');

  const HANDLE_ICONS: Record<string, string> = { email: '✉', telegramid: '✈', telegramusername: '@', phone: '☏' };

  async function loadList() {
    try { cards = await get<PersonCard[]>('/api/people?limit=80'); } catch (e: any) { error = e.message; cards = []; }
  }
  async function loadDetail(pid: string) {
    detail = null;
    try {
      detail = await get<PersonDetail>(`/api/person/${pid}`);
      roles = [...detail.card.roles];
      notes = detail.notes || '';
    } catch (e: any) { error = e.message; }
  }
  async function save() {
    if (!id) return;
    try { await post(`/api/person/${id}/relationship`, { roles, notes }); } catch (e: any) { error = e.message; }
  }
  function addRole() {
    const r = newRole.trim();
    if (!r) return;
    roles.push(r); newRole = ''; save();
  }
  function removeRole(r: string) { roles = roles.filter((x) => x !== r); save(); }

  $effect(() => { loadList(); });
  $effect(() => { if (id) loadDetail(id); else detail = null; });

  function bars(months: number[]) {
    const max = Math.max(1, ...months);
    return months.map((n) => Math.max(2, Math.round((n / max) * 28)));
  }
</script>

<!-- Design 07: a relationship is the person, the roles you gave them, the
     notes you wrote, and arithmetic over what passed between you. Nothing
     on this page is the model's opinion. -->
<section class="pane people-pane" class:has-detail={!!id}>
  <ol class="people">
    {#if error && !cards?.length}<li><Empty text={error} error /></li>{/if}
    {#if cards && !cards.length}<li><Empty text={t('people.empty')} /></li>{/if}
    {#each cards ?? [] as card (card.id)}
      <li>
       <button type="button" class="person-card" class:is-on={card.id === id} onclick={() => go(`/people/${card.id}`)}>
        <span class="avatar">{initials(card.name)}</span>
        <div class="person-body">
          <div class="person-head">
            <span class="person-name">{card.name}</span>
            {#each card.roles as r}<span class="role">{r}</span>{/each}
          </div>
          <div class="person-line">
            <span class="count in">↓ {card.from_them}</span>
            <span class="count out">↑ {card.to_them}</span>
            {#if card.last_at}<span class="faint">{t('people.last', { at: card.last_at })}</span>{/if}
            <span class="faint">{card.connectors.map(connectorName).join(' · ')}</span>
          </div>
        </div>
       </button>
      </li>
    {/each}
  </ol>

  {#if id}
    <div class="person">
      <button type="button" class="back" onclick={() => go('/people')}><Icon name="back" size={16} />{t('people.back')}</button>
      {#if !detail}<Empty text={error || t('loading')} error={!!error} />
      {:else}
        <div class="person-detail-head">
          <span class="avatar big">{initials(detail.card.name)}</span>
          <div class="person-title-box">
            <h2 class="person-title">{detail.card.name}</h2>
            <div class="roles">
              {#each roles as r}
                <button type="button" class="role removable" title="remove" onclick={() => removeRole(r)}>{r}</button>
              {/each}
              <input class="role-input" bind:value={newRole} placeholder={t('people.addrole')} onkeydown={(e) => { if (e.key === 'Enter') addRole(); }} />
            </div>
            <div class="handles">
              {#each detail.card.handles as h}<span class="handle" class:inferred={h.inferred}>{HANDLE_ICONS[h.kind] || ''} {h.value}</span>{/each}
            </div>
          </div>
        </div>
        <div class="tiles">
          <div class="tile"><span class="tile-value">{detail.stats.from_them}</span><span class="tile-label">{t('people.fromthem')}</span></div>
          <div class="tile"><span class="tile-value">{detail.stats.to_them}</span><span class="tile-label">{t('people.fromyou')}</span></div>
          <div class="tile"><span class="tile-value">{hours(detail.stats.reply_hours)}</span><span class="tile-label">{t('people.replytime')}</span>
            <span class="tile-note">{detail.stats.reply_hours == null ? t('people.nomeasure') : t('people.median')}</span></div>
          <div class="tile"><span class="tile-value">{detail.stats.first_at || '—'}</span><span class="tile-label">{t('people.since')}</span>
            {#if detail.stats.last_at}<span class="tile-note">{t('people.last', { at: detail.stats.last_at })}</span>{/if}</div>
          <div class="tile"><span class="tile-value">{detail.stats.language || '—'}</span><span class="tile-label">{t('people.language')}</span></div>
        </div>
        <div class="chart">
          <span class="chart-label">{t('people.months')}</span>
          <div class="months">
            {#each bars(detail.stats.months) as h, i}
              <span class="month" class:now={i === detail.stats.months.length - 1} style="height:{h}px" title={String(detail.stats.months[i])}></span>
            {/each}
          </div>
        </div>
        <h3 class="group-title">{t('people.notes')}</h3>
        <textarea class="notes" bind:value={notes} onblur={save} placeholder={t('people.notes.hint')}></textarea>
        {#if detail.commitments.length}
          <h3 class="group-title">{t('people.promises', { n: detail.commitments.length })}</h3>
          <ol class="rows">{#each detail.commitments as c (c.id)}<CommitmentRow {c} />{/each}</ol>
        {/if}
        <h3 class="group-title">{t('people.recent')}</h3>
        <ol class="rows">
          {#each detail.recent as r (r.id)}<ItemRow row={r} />{/each}
          {#if !detail.recent.length}<li><Empty text={t('point.nothing')} /></li>{/if}
        </ol>
      {/if}
    </div>
  {/if}
</section>
