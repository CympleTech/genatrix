<script lang="ts">
  import { get, post, type Action } from '../lib/api';
  import { cardValue, kindWord, statusLine } from '../lib/format';
  import { t } from '../lib/i18n';
  import SourceChip from './SourceChip.svelte';
  import { refreshPending } from '../lib/status';

  let { action, ondone, onsettled }: {
    action: Action;
    ondone?: () => void;
    /** Called once when an approved action has an outcome: sent, not sent, or unknown. */
    onsettled?: (a: Action) => void;
  } = $props();
  // The version and nonce this card holds: what approving must present back.
  // The card owns its copy from here on; a fresh list makes fresh cards.
  // svelte-ignore state_referenced_locally
  let current = $state(action);
  // svelte-ignore state_referenced_locally
  let draft = $state(action.draft);
  let busy = $state(false);
  let message = $state('');
  let error = $state('');
  let askingReason = $state(false);
  let reason = $state('');
  let settled = $state<string>(''); // what happened on this card

  async function saveEditIfAny() {
    if (draft !== current.draft) {
      current = await post<Action>(`/api/action/${action.id}/edit`, { payload: draft });
      draft = current.draft;
    }
  }
  async function approve() {
    busy = true; error = '';
    try {
      await saveEditIfAny();
      const done = await post<Action>(`/api/action/${action.id}/approve`, {
        version: current.version, payload_hash: current.payload_hash, nonce: current.nonce,
      });
      current = done; refreshPending(); ondone?.();
      follow();
    } catch (e: any) { error = e.message; busy = false; }
  }

  // After approval the connector takes the action within seconds, sends it
  // and reports. Ask how it went until it has gone one of the three ways
  // design 03 allows, so the card says "sent" without a reload. Quickly at
  // first, then slowly, and not past the ten minutes after which the core
  // itself stops waiting and calls the outcome unknown.
  let alive = true;
  $effect(() => () => { alive = false; });
  async function follow() {
    const started = Date.now();
    while (alive && Date.now() - started < 11 * 60_000) {
      await new Promise((r) => setTimeout(r, Date.now() - started < 60_000 ? 2500 : 15_000));
      if (!alive) return;
      try {
        const now = await get<Action>(`/api/action/${action.id}`);
        current = now;
        if (now.status !== 'approved') { refreshPending(); onsettled?.(now); return; }
      } catch { /* the next try will do */ }
    }
  }
  // A card that arrives already approved, on the Approvals page or Today,
  // follows it too.
  // svelte-ignore state_referenced_locally
  if (action.status === 'approved') follow();

  async function decline() {
    if (!askingReason) { askingReason = true; return; }
    busy = true; error = '';
    try {
      const done = await post<Action>(`/api/action/${action.id}/decline`, { reason });
      current = done; settled = t('action.declined', { reason: done.status_detail }); refreshPending(); ondone?.();
    } catch (e: any) { error = e.message; busy = false; }
  }
  async function withdraw() {
    busy = true; error = '';
    try {
      const done = await post<Action>(`/api/action/${action.id}/decline`, { reason: 'withdrawn before sending' });
      current = done; settled = t('action.withdrawn'); ondone?.();
    } catch (e: any) { error = e.message; busy = false; }
  }
  const editable = $derived(current.status === 'pending' && !settled);
  // A dispatch's task is personal unless the user says otherwise; saying so
  // is a new version, so an earlier approval does not carry over (design 13).
  async function setPublic(e: Event) {
    const on = (e.target as HTMLInputElement).checked;
    busy = true; error = '';
    try {
      await saveEditIfAny();
      current = await post<Action>(`/api/action/${action.id}/level`, { public: on });
      draft = current.draft;
    } catch (err: any) { error = err.message; (e.target as HTMLInputElement).checked = !on; }
    busy = false;
  }
</script>

<!-- Design 06: the one screen where principle five is kept. Evidence before
     the draft; the draft editable in place; the approve button names the
     consequence; a decline wants a reason. -->
<li class="row is-open action {current.status}">
  <div class="meta">
    <span class="who">{kindWord(current.kind)} → {current.target}</span>
    <span class="faint">{statusLine(current)}</span>
  </div>
  {#if current.evidence.length}
    <h4 class="card-label">{t('action.evidence')}</h4>
    <div class="sources">{#each current.evidence as s}<SourceChip src={s} />{/each}</div>
  {/if}
  {#if current.rationale}
    <h4 class="card-label">{t('action.why')}</h4>
    <p class="rationale">{current.rationale}</p>
  {/if}
  {#if current.card}
    <!-- An agent's proposal: its data in one fixed template, nothing of the
         agent's own drawn here (design 11, ruling 4). -->
    <h4 class="card-label">{current.card.title}</h4>
    <dl class="card-fields">
      {#each current.card.fields as f}
        <dt>{f.label}</dt>
        <dd class:money={f.value.type === 'money'}>{cardValue(f.value)}</dd>
      {/each}
    </dl>
    <p class="note">{t('action.by_agent')}</p>
  {:else}
    {#if current.facts?.length}
      <!-- Work for executors: where the words and the code go, in one fixed
           layout, above the words themselves (design 13). -->
      <dl class="card-fields">
        {#each current.facts as f}<dt>{f.label}</dt><dd>{f.value}</dd>{/each}
      </dl>
    {/if}
    <h4 class="card-label">{t('action.draft')}{current.versions > 1 ? ' · ' + t('action.version', { v: current.version }) : ''}</h4>
    <textarea class="draft" bind:value={draft} readonly={!editable || !current.editable}></textarea>
    {#if current.kind === 'dispatch'}
      <label class="quiet-toggle">
        <input type="checkbox" checked={current.level === 'public'} disabled={!editable || busy} onchange={setPublic} />
        {t('work.public')}
      </label>
      <p class="note">{t('work.card_note')}</p>
    {:else}
      <p class="note">{t('action.drafted', { where: current.drafted, account: current.account })}</p>
    {/if}
  {/if}
  {#if current.result}
    <h4 class="card-label">{t('action.sent')}</h4>
    <div class="sources"><SourceChip src={current.result} /></div>
  {/if}
  {#if settled}
    <p class="note">{settled}</p>
  {:else if current.status === 'pending'}
    <div class="chooser">
      <button type="button" class="choose primary" disabled={busy} onclick={approve}>{current.card ? t('action.approve_agent') : t('action.approve')}</button>
      <button type="button" class="choose lowers" disabled={busy} onclick={decline}>{t('action.decline')}</button>
      {#if askingReason}
        <input class="reason" bind:value={reason} placeholder={t('action.reason')} onkeydown={(e) => { if (e.key === 'Enter') decline(); }} />
      {/if}
      {#if error}<span class="error">{error}</span>{/if}
    </div>
  {:else if current.status === 'approved'}
    <div class="chooser">
      <span class="note sending">{current.card ? t('action.approved_agent') : t('action.approved')}</span>
      {#if current.can_withdraw}
        <button type="button" class="choose lowers" disabled={busy} onclick={withdraw}>{t('action.withdraw')}</button>
      {/if}
      {#if error}<span class="error">{error}</span>{/if}
    </div>
  {/if}
</li>
