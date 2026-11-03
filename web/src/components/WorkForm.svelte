<script lang="ts">
  import { get, post, type Action, type TaskDetail, type WorkProject, type WorkRecipe } from '../lib/api';
  import { t } from '../lib/i18n';
  import { go } from '../lib/router';
  import ActionCard from './ActionCard.svelte';

  // Design 13, "第一层：派单", within one space (design 14): the projects
  // offered are this space's only. Drafting makes a pending action like
  // any other: the card below the form is the one the approvals page shows.
  // Work from a task carries the task along, and the core checks its
  // account is in this space, when drafted and again when it starts.
  let { projects, recipes, fromTask = null, here }: {
    projects: WorkProject[]; recipes: WorkRecipe[]; fromTask?: string | null; here: string;
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

  $effect(() => {
    if (!projects.some((p) => p.name === project)) project = projects[0]?.name ?? '';
  });
  $effect(() => {
    if (!recipes.some((r) => r.name === recipe)) recipe = recipes[0]?.name ?? '';
  });
  $effect(() => {
    const which = fromTask;
    source = null;
    if (!which) return;
    get<TaskDetail>(`/api/task/${which}`).then((d) => {
      source = d;
      task = [d.name, d.description, d.url ?? ''].filter((x) => x.trim()).join('\n\n');
    }).catch((e: any) => { error = e.message; });
  });

  const chosenProject = $derived(projects.find((p) => p.name === project) ?? null);
  const chosenRecipe = $derived(recipes.find((r) => r.name === recipe) ?? null);

  async function draft() {
    drafting = true; error = ''; notice = ''; drafted = null;
    try {
      drafted = await post<Action>('/api/work/draft', { project, recipe, task, public: isPublic, source: fromTask });
      notice = t('work.drafted');
      task = '';
      if (fromTask) go(here, true);
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

{#if fromTask}
  <p class="note">
    {#if source}{t('work.fromtask', { title: source.name })}{:else}{t('loading')}{/if}
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
    <!-- What pressing a button did shows here, beside the button. -->
    <div aria-live="polite">
      {#if error}<p class="note error">{error}</p>{/if}
      {#if notice}<p class="note">{notice}</p>{/if}
    </div>
  </div>
  {#if drafted}<ol class="rows cards"><ActionCard action={drafted} /></ol>{/if}
{/if}
