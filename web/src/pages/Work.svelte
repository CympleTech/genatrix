<script lang="ts">
  import { get, post, type Action, type WorkDetail, type WorkEvent, type WorkOverview } from '../lib/api';
  import { t } from '../lib/i18n';
  import { go } from '../lib/router';
  import ActionCard from '../components/ActionCard.svelte';
  import Empty from '../components/Empty.svelte';

  // Design 13, "界面": hand work to executors, watch it, stop it, read what
  // it changed. Drafting makes a pending action like any other: the card
  // below the form is the same one the approvals page shows. What the
  // executors said is shown as they said it, as material, never as advice
  // from Genatrix.
  let { id }: { id: string | null } = $props();

  let overview = $state<WorkOverview | null>(null);
  let error = $state('');
  let notice = $state('');
  let project = $state('');
  let recipe = $state('implement-and-review');
  let task = $state('');
  let isPublic = $state(false);
  let drafting = $state(false);
  let drafted = $state<Action | null>(null);
  let definition = $state('');

  async function load() {
    try {
      overview = await get<WorkOverview>('/api/work');
      if (!project && overview.projects.length) project = overview.projects[0].name;
      error = '';
    } catch (e: any) { error = e.message; }
  }
  $effect(() => { if (!id) load(); });

  const chosenProject = $derived(overview?.projects.find((p) => p.name === project) ?? null);
  const chosenRecipe = $derived(overview?.recipes.find((r) => r.name === recipe) ?? null);

  async function draft() {
    drafting = true; error = ''; notice = ''; drafted = null;
    try {
      drafted = await post<Action>('/api/work/draft', { project, recipe, task, public: isPublic });
      notice = t('work.drafted');
      task = '';
    } catch (e: any) { error = e.message; }
    drafting = false;
  }
  // The local model fills the form in from a sentence; nothing is drafted
  // until you press draft (design 13, "调度").
  let suggesting = $state(false);
  async function suggest() {
    suggesting = true; error = '';
    try {
      const s = await post<{ project: string; recipe: string; task: string }>('/api/work/suggest', { text: task });
      project = s.project; recipe = s.recipe; task = s.task;
    } catch (e: any) { error = e.message; }
    suggesting = false;
  }
  async function fileProject() {
    error = ''; notice = '';
    try {
      await post('/api/work/define', { kind: 'project', text: definition });
      definition = ''; notice = t('work.filed');
      await load();
    } catch (e: any) { error = e.message; }
  }

  // One dispatch: its record, followed while it runs.
  let detail = $state<WorkDetail | null>(null);
  let events = $state<WorkEvent[]>([]);
  let diff = $state<string | null>(null);
  let alive = true;
  $effect(() => () => { alive = false; });
  async function follow(which: string) {
    events = []; detail = null; diff = null;
    let after = { seq: 0, n: 0 };
    while (alive && id === which) {
      try {
        const d = await get<WorkDetail>(`/api/work/${which}?seq=${after.seq}&n=${after.n}`);
        if (id !== which) return;
        detail = d;
        if (d.events.length) {
          events = [...events, ...d.events];
          const last = d.events[d.events.length - 1];
          after = { seq: last.seq, n: last.n };
        }
        if (d.dispatch.state !== 'running') return;
      } catch (e: any) { error = e.message; return; }
      await new Promise((r) => setTimeout(r, 2000));
    }
  }
  $effect(() => { if (id) follow(id); });

  async function stop() {
    if (!id) return;
    try { await post(`/api/work/${id}/stop`); } catch (e: any) { error = e.message; }
  }
  // A push GitHub refused (or you declined) is offered again as a new card.
  // Its failure shows beside the button; `error` would replace the page.
  let offered = $state<Action | null>(null);
  let offerError = $state('');
  async function pushAgain() {
    if (!id) return;
    offerError = '';
    try {
      offered = await post<Action>(`/api/work/${id}/push-again`);
      if (detail) detail = { ...detail, push_again: false };
    } catch (e: any) { offerError = e.message; }
  }
  async function toggleDiff() {
    if (diff !== null) { diff = null; return; }
    try { diff = (await get<{ diff: string }>(`/api/work/${id}/diff`)).diff; } catch (e: any) { error = e.message; }
  }
  const when = (ms: number) => new Date(ms).toLocaleString();
  const stateWord = (s: string) => t(`work.state.${s}`);
</script>

<section class="pane work">
  {#if id}
    <button type="button" class="back" onclick={() => go('/work')}>← {t('work.back')}</button>
    {#if error}<Empty text={error} error />
    {:else if !detail}<Empty text={t('loading')} />
    {:else}
      {@const d = detail.dispatch}
      <h2 class="group-title">{d.project} · {d.recipe}</h2>
      <p class="note">
        <span class="state {d.state}">{stateWord(d.state)}</span> · {d.branch} · {when(d.started_ms)}
        {#if d.review_passed === true} · {t('work.review.pass')}{:else if d.review_passed === false} · {t('work.review.changes')}{/if}
      </p>
      {#if d.detail}<p class="note">{d.detail}</p>{/if}
      <div class="chooser">
        {#if d.state === 'running'}<button type="button" class="choose lowers" onclick={stop}>{t('work.stop')}</button>{/if}
        {#if d.tip}<button type="button" class="choose" onclick={toggleDiff}>{diff === null ? t('work.diff') : t('work.diff.hide')}</button>{/if}
        {#if detail.push_again}<button type="button" class="choose" onclick={pushAgain}>{t('work.push_again')}</button>{/if}
      </div>
      {#if offerError}<p class="note error">{offerError}</p>{/if}
      {#if offered}<ol class="rows cards"><ActionCard action={offered} /></ol>{/if}
      {#if diff !== null}<pre class="diff">{diff}</pre>{/if}

      <h4 class="card-label">{t('work.steps')}</h4>
      <ol class="rows">
        {#each detail.steps as s (s.seq)}
          <li class="row">
            <div class="meta">
              <span class="who">{s.step}{s.round > 1 ? ' · ' + t('work.round', { n: s.round }) : ''}</span>
              <span class="faint">{s.executor} · {s.role} · {s.state}</span>
            </div>
            {#if s.verdict}
              <p class="text">{s.verdict.pass ? t('work.review.pass') : t('work.review.changes')}{s.verdict.notes ? ': ' + s.verdict.notes : ''}</p>
            {/if}
            {#if s.commit_id}<p class="faint">{t('work.commit', { c: s.commit_id.slice(0, 10) })}</p>{/if}
            {#if s.error}<p class="error">{s.error}</p>{/if}
          </li>
        {/each}
      </ol>

      <h4 class="card-label">{t('work.said')}</h4>
      <p class="note">{t('work.said.note')}</p>
      <ol class="said">
        {#each events as e (e.seq + ':' + e.n)}
          <li class="said-{e.kind}"><span class="faint">#{e.seq} {e.kind}</span> {e.text}</li>
        {/each}
      </ol>
    {/if}
  {:else}
    <p class="note">{t('work.lede')}</p>
    {#if error && !overview?.projects.length}<p class="note error">{error}</p>{/if}
    {#if notice && !overview?.projects.length}<p class="note">{notice}</p>{/if}
    {#if !overview}<Empty text={t('loading')} />
    {:else}
      {#if !overview.github}<p class="note">{t('work.github.off')}</p>{/if}
      {#if !overview.projects.length}<p class="note">{t('work.none')}</p>{/if}

      {#if overview.projects.length}
        <h2 class="group-title">{t('work.new')}</h2>
        <div class="work-form">
          <div class="field">
            <span class="field-label" id="work-project">{t('work.project')}</span>
            <div class="chooser" role="radiogroup" aria-labelledby="work-project">
              {#each overview.projects as p}
                <button type="button" class="choose" role="radio" aria-checked={project === p.name} class:is-on={project === p.name} onclick={() => (project = p.name)}>{p.name}</button>
              {/each}
            </div>
            {#if chosenProject}
              <p class="field-note">{chosenProject.github ?? '—'} · {chosenProject.executors.join(', ')}</p>
            {/if}
          </div>
          <div class="field">
            <span class="field-label" id="work-recipe">{t('work.recipe')}</span>
            <div class="chooser" role="radiogroup" aria-labelledby="work-recipe">
              {#each overview.recipes as r}
                <button type="button" class="choose" role="radio" aria-checked={recipe === r.name} class:is-on={recipe === r.name} onclick={() => (recipe = r.name)}>{r.title}</button>
              {/each}
            </div>
            {#if chosenRecipe}
              <p class="field-note">{chosenRecipe.steps.map((s) => `${s.id} (${s.role}, ${s.executor})`).join(' → ')}{chosenRecipe.rounds ? ` · ≤ ${chosenRecipe.rounds}` : ''}</p>
            {/if}
          </div>
          <label>{t('work.task')}
            <textarea class="draft" bind:value={task} placeholder={t('work.task.placeholder')}></textarea>
          </label>
          <label class="quiet-toggle"><input type="checkbox" bind:checked={isPublic} /> {t('work.public')}</label>
          <div class="chooser">
            <button type="button" class="choose primary" disabled={drafting || !task.trim() || !project} onclick={draft}>{drafting ? t('work.drafting') : t('work.draft')}</button>
            <button type="button" class="choose" disabled={suggesting || !task.trim()} onclick={suggest}>{suggesting ? t('work.suggesting') : t('work.suggest')}</button>
          </div>
          <!-- What pressing a button did shows here, beside the button, not at the top of the page. -->
          <div aria-live="polite">
            {#if error}<p class="note error">{error}</p>{/if}
            {#if notice}<p class="note">{notice}</p>{/if}
          </div>
        </div>
        {#if drafted}<ol class="rows cards"><ActionCard action={drafted} /></ol>{/if}
      {/if}

      <h2 class="group-title">{t('work.recent')}</h2>
      {#if !overview.dispatches.length}<Empty text={t('work.empty')} />
      {:else}
        <ol class="rows">
          {#each overview.dispatches as d (d.id)}
            <li class="row">
              <button type="button" class="linkish" onclick={() => go('/work/' + d.id)}>
                <span class="who">{d.project} · {d.recipe}</span>
              </button>
              <span class="faint"> · <span class="state {d.state}">{stateWord(d.state)}</span> · {when(d.started_ms)}</span>
              {#if d.detail}<p class="faint">{d.detail}</p>{/if}
            </li>
          {/each}
        </ol>
      {/if}

      {#if overview.local}
        <h2 class="group-title">{t('work.add_project')}</h2>
        <p class="note">{t('work.add_project.note')}</p>
        <textarea class="notes" bind:value={definition} placeholder={'name = "my-app"\nrepo = "~/code/my-app"\ngithub = "me/my-app"\nexecutors = ["claude-code", "codex"]\ncommands = ["npm test"]'}></textarea>
        <div class="chooser"><button type="button" class="choose" disabled={!definition.trim()} onclick={fileProject}>{t('work.file')}</button></div>
      {/if}
    {/if}
  {/if}
</section>
