<script lang="ts">
  import { untrack } from 'svelte';
  import { get, post, type Action, type IssueSource, type TaskDetail, type WorkProject, type WorkRecipe } from '../lib/api';
  import { t } from '../lib/i18n';
  import { go } from '../lib/router';
  import ActionCard from './ActionCard.svelte';

  // Design 13, "第一层：派单", within one space (design 14): the projects
  // offered are this space's only. Drafting makes a pending action like
  // any other: the card below the form is the one the approvals page shows.
  // Work from a task carries the task along, and the core checks its
  // account is in this space, when drafted and again when it starts. Work
  // from a GitHub issue (design 13 v0.7) is the same: the issue goes as
  // written, and the core checks its repository is this space's.
  let { projects, recipes, executors = [], fromTask = null, fromIssue = null, here }: {
    projects: WorkProject[]; recipes: WorkRecipe[]; executors?: { name: string; vendor: string }[];
    fromTask?: string | null; fromIssue?: IssueSource | null; here: string;
  } = $props();

  let project = $state('');
  let recipe = $state('implement-and-review');
  let task = $state('');
  let isPublic = $state(false);
  let drafting = $state(false);
  let drafted = $state<Action | null>(null);
  let error = $state('');
  let notice = $state('');
  let source = $state<TaskDetail | null>(null);
  // From a task, the task itself goes as written and you only add to it.
  let extra = $state('');
  // What the work comes from, quoted: a task, or an issue.
  const quote = $derived(fromIssue
    ? { name: `#${fromIssue.number} ${fromIssue.title}`, description: fromIssue.body, url: fromIssue.url, open: t('work.issue.open') }
    : source ? { name: source.name, description: source.description, url: source.url ?? '', open: t('work.task.open') } : null);
  const from = $derived(!!fromTask || !!fromIssue);
  const sourceId = $derived(fromIssue?.source ?? fromTask);
  const fromSource = $derived(quote ? [quote.name, quote.description, quote.url].filter((x) => x.trim()).join('\n\n') : '');
  // An issue starts on its own project.
  $effect(() => {
    const p = fromIssue?.project;
    extra = '';
    if (p && untrack(() => projects.some((x) => x.name === p))) project = p;
  });

  $effect(() => {
    if (!projects.some((p) => p.name === project)) project = projects[0]?.name ?? '';
  });
  $effect(() => {
    if (!recipes.some((r) => r.name === recipe)) recipe = recipes[0]?.name ?? '';
  });
  $effect(() => {
    const which = fromTask;
    source = null; extra = '';
    if (!which) return;
    get<TaskDetail>(`/api/task/${which}`).then((d) => {
      if (fromTask === which) source = d;
    }).catch((e: any) => { error = e.message; });
  });

  const chosenProject = $derived(projects.find((p) => p.name === project) ?? null);
  const chosenRecipe = $derived(recipes.find((r) => r.name === recipe) ?? null);

  // Design 13 v0.5: who does each step is yours to choose, among the
  // executors the project lets see its code. The recipe's choice is where
  // each starts; a step whose executor the project does not allow starts
  // on the first one it does.
  const choices = $derived((chosenProject?.executors ?? []).filter((n) => !executors.length || executors.some((e) => e.name === n)));
  const vendorOf = (n: string) => executors.find((e) => e.name === n)?.vendor ?? '';
  let assign = $state<Record<string, string>>({});
  $effect(() => {
    const next: Record<string, string> = {};
    const before = untrack(() => assign);
    for (const st of chosenRecipe?.steps ?? []) {
      const kept = before[st.id];
      next[st.id] = kept && choices.includes(kept) ? kept : choices.includes(st.executor) ? st.executor : (choices[0] ?? st.executor);
    }
    assign = next;
  });
  const stepWord = (id: string) => { const k = `work.step.${id}`; const w = t(k); return w === k ? id : w; };

  // What goes to the executor: the task, then what you added, under a heading.
  const text = $derived(from
    ? [fromSource, extra.trim() ? `${t('work.extra.heading')}\n${extra.trim()}` : ''].filter(Boolean).join('\n\n')
    : task);

  async function draft() {
    drafting = true; error = ''; notice = ''; drafted = null;
    try {
      drafted = await post<Action>('/api/work/draft', { project, recipe, task: text, public: isPublic, source: sourceId, assign });
      notice = t('work.drafted');
      task = ''; extra = '';
      if (from) go(here, true);
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
      if (projects.some((p) => p.name === s.project)) project = s.project;
      recipe = s.recipe; task = s.task;
    } catch (e: any) { error = e.message; }
    suggesting = false;
  }
</script>

{#if from}
  <p class="note">
    {#if quote}{t('work.fromtask', { title: quote.name })}{:else}{t('loading')}{/if}
    · <button type="button" class="linkish" onclick={() => go(here, true)}>{t('work.fromtask.clear')}</button>
  </p>
{/if}
{#if !projects.length}
  <p class="note">{t('spaces.noprojects')}</p>
{:else}
  <div class="work-form">
    <div class="field">
      <span class="field-label" id="work-project">{t('work.project')}</span>
      <div class="chooser" role="radiogroup" aria-labelledby="work-project">
        {#each projects as p}
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
        {#each recipes as r}
          <button type="button" class="choose" role="radio" aria-checked={recipe === r.name} class:is-on={recipe === r.name} onclick={() => (recipe = r.name)}>{r.title}</button>
        {/each}
      </div>
      {#if chosenRecipe}
        <ol class="step-picks">
          {#each chosenRecipe.steps as st, i (st.id)}
            <li>
              <span class="step-n">{i + 1}</span>
              <span class="step-name">{stepWord(st.id)}<span class="faint"> · {st.role}</span>{#if st.produces === 'document' && st.file}<span class="step-file">{t('work.step.file', { file: st.file })}</span>{/if}</span>
              {#if choices.length}
                <select bind:value={assign[st.id]} aria-label={t('work.step.who', { step: st.id })}>
                  {#each choices as c (c)}<option value={c}>{c}{vendorOf(c) ? ` · ${vendorOf(c)}` : ''}</option>{/each}
                </select>
              {:else}<span class="faint">{st.executor}</span>{/if}
            </li>
          {/each}
        </ol>
        <p class="field-note">{t('work.step.note')}{chosenRecipe.rounds ? ` ${t('work.step.rounds', { n: chosenRecipe.rounds })}` : ''}</p>
      {/if}
    </div>
    {#if from}
      <div class="field">
        <span class="field-label">{fromIssue ? t('work.issue.from') : t('work.task.from')}</span>
        {#if quote}
          <div class="task-quote">
            <p class="task-quote-title">{quote.name}</p>
            {#if quote.description.trim()}<p class="task-quote-body">{quote.description}</p>{/if}
            {#if quote.url}<a href={quote.url} target="_blank" rel="noopener noreferrer">{quote.open}</a>{/if}
          </div>
        {:else}<p class="field-note">{t('loading')}</p>{/if}
      </div>
      <label>{t('work.extra')}
        <textarea class="draft" bind:value={extra} placeholder={t('work.extra.placeholder')}></textarea>
      </label>
    {:else}
      <label>{t('work.task')}
        <textarea class="draft" bind:value={task} placeholder={t('work.task.placeholder')}></textarea>
      </label>
    {/if}
    <label class="quiet-toggle"><input type="checkbox" bind:checked={isPublic} /> {t('work.public')}</label>
    <div class="chooser">
      <button type="button" class="choose primary" disabled={drafting || !text.trim() || !project || (from && !quote)} onclick={draft}>{drafting ? t('work.drafting') : t('work.draft')}</button>
      {#if !from}<button type="button" class="choose" disabled={suggesting || !task.trim()} onclick={suggest}>{suggesting ? t('work.suggesting') : t('work.suggest')}</button>{/if}
    </div>
    <!-- What pressing a button did shows here, beside the button. -->
    <div aria-live="polite">
      {#if error}<p class="note error">{error}</p>{/if}
      {#if notice}<p class="note">{notice}</p>{/if}
    </div>
  </div>
  {#if drafted}<ol class="rows cards"><ActionCard action={drafted} /></ol>{/if}
{/if}
