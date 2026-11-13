<script lang="ts">
  import { untrack } from 'svelte';
  import { get, post, type Dispatch, type Party, type WorkOverview, type WorkProject } from '../lib/api';
  import { t } from '../lib/i18n';
  import { go, route } from '../lib/router';
  import Empty from '../components/Empty.svelte';
  import WorkForm from '../components/WorkForm.svelte';
  import SpaceForm from '../components/SpaceForm.svelte';
  import ProjectForm from '../components/ProjectForm.svelte';

  // Design 06 v0.14 and v0.17, design 14: a space is where work is done and its
  // record kept. It is a view over the one base, cut by the space's
  // accounts and projects: its tasks are the same conversations Chats
  // shows, its work the same dispatches.
  let { name }: { name: string | null } = $props();

  interface Card { name: string; title: string; accounts: number; projects: number; todo: number; active: number }
  interface AccountLine { key: string; kind: string; id: string; state: string; text: string }
  interface Done extends Dispatch { sent: number; to: string[] }
  interface Home {
    name: string; title: string; accounts: AccountLine[]; projects: WorkProject[];
    todo: Party[]; active: Dispatch[]; record: Done[]; approvals: number;
    github: 'own' | 'default' | 'none'; manage: boolean;
  }

  let cards = $state<Card[] | null>(null);
  let home = $state<Home | null>(null);
  let work = $state<WorkOverview | null>(null);
  let error = $state('');
  let notice = $state('');
  // Which form is open: adding a space, adding a project, changing this space.
  let open = $state<'' | 'space' | 'project' | 'edit'>('');

  async function load() {
    error = '';
    try {
      if (name) {
        const which = name;
        const [h, w] = await Promise.all([get<Home>(`/api/spaces/${which}`), get<WorkOverview>('/api/work')]);
        if (name !== which) return;
        home = h; work = w;
      } else {
        cards = await get<Card[]>('/api/spaces');
        work = await get<WorkOverview>('/api/work');
      }
    } catch (e: any) { error = e.message; }
  }
  // Design 06 v0.17: a space is an overview and four pages under it, so no
  // one page carries everything. The page is the path's third part.
  const TABS = ['', 'todo', 'new', 'work', 'setup'] as const;
  type Tab = (typeof TABS)[number];
  const tab = $derived<Tab>((TABS as readonly string[]).includes($route.parts[2] ?? '') ? ($route.parts[2] ?? '') as Tab : '');
  function goTab(k: Tab) { notice = ''; go(`/spaces/${name}${k ? '/' + k : ''}`); }
  // A new space starts empty; moving between its pages keeps what is shown
  // and asks again, so the counts are fresh.
  $effect(() => {
    name; tab;
    untrack(() => { if (home?.name !== name) home = null; open = ''; });
    load();
  });

  const fromTask = $derived($route.query.get('task'));
  // Stop work from the list, without opening it first. It winds down in a
  // few seconds; the list is asked again until it has.
  let stoppingIds = $state<string[]>([]);
  async function stopWork(id: string) {
    stoppingIds = [...stoppingIds, id]; error = '';
    try {
      await post(`/api/work/${id}/stop`);
      for (let i = 0; i < 15 && home?.active.some((d) => d.id === id); i++) {
        await new Promise((r) => setTimeout(r, 1500));
        await load();
      }
    } catch (e: any) { error = e.message; }
    stoppingIds = stoppingIds.filter((x) => x !== id);
  }
  // Dispatching a to-do opens the new-work page with the task in it.
  function dispatchTask(id: string) {
    if (home) go(`/spaces/${home.name}/new?task=${id}`);
  }

  async function filed(what: 'space' | 'project' | 'edit', saved: string) {
    open = ''; error = '';
    notice = t(what === 'project' ? 'projectform.added' : what === 'space' ? 'spaceform.added' : 'work.filed', { name: saved });
    if (what === 'space') { go('/spaces/' + saved); return; }
    await load();
  }
  // The overview: four numbers, each opening where it is counted, and what
  // needs you, each line opening where you deal with it.
  const stats = $derived(home ? [
    { key: 'todo', n: home.todo.length, hot: false, go: () => goTab('todo') },
    { key: 'active', n: home.active.length, hot: false, go: () => goTab('work') },
    { key: 'approvals', n: home.approvals, hot: true, go: () => go('/approvals') },
    { key: 'done', n: home.record.length, hot: false, go: () => goTab('work') },
  ] : []);
  const needs = $derived.by(() => {
    if (!home) return [];
    const h = home;
    const out: { key: string; text: string; bad: boolean; go: () => void }[] = [];
    for (const d of h.active.filter((d) => d.state === 'waiting'))
      out.push({ key: 'w' + d.id, text: t('spaces.needs.answer', { what: `${d.project} · ${d.recipe}` }), bad: false, go: () => go('/work/' + d.id) });
    if (h.approvals) out.push({ key: 'approvals', text: t('spaces.needs.approvals', { n: h.approvals }), bad: false, go: () => go('/approvals') });
    for (const a of h.accounts.filter((a) => live(a.state) === 'bad'))
      out.push({ key: 'a' + a.key, text: t('spaces.needs.account', { id: a.id, text: a.text }), bad: true, go: () => goTab('setup') });
    if (!h.projects.length) out.push({ key: 'noproject', text: t('spaces.needs.noproject'), bad: true, go: () => goTab('setup') });
    else if (h.github === 'none' && h.projects.some((p) => p.github)) out.push({ key: 'github', text: t('spaces.needs.github'), bad: true, go: () => goTab('setup') });
    return out;
  });
  const thisSpace = $derived(work?.workspaces.find((w) => w.name === home?.name) ?? null);

  // The built-in space's title is the page's own word for it.
  const titled = (n: string, title: string) => (n === 'personal' && title === 'Personal' ? t('spaces.personal') : title);
  const when = (ms: number) => new Date(ms).toLocaleString();
  const stateWord = (s: string) => t(`work.state.${s}`);
  const live = (s: string) => s === 'live' ? 'ok' : s === 'missing' || s === 'stopped' || s === 'needs_user' ? 'bad' : '';
</script>

<section class="pane spaces">
  {#if error}<p class="note error">{error}</p>{/if}
  {#if notice}<p class="note">{notice}</p>{/if}

  {#if !name}
    <p class="note">{t('spaces.lede')}</p>
    {#if !cards}<Empty text={t('loading')} />
    {:else}
      <ol class="rows space-cards">
        {#each cards as c (c.name)}
          <li class="row">
            <button type="button" class="space-card" onclick={() => go('/spaces/' + c.name)}>
              <span class="space-title">{titled(c.name, c.title)}</span>
              <span class="space-counts">
                {#if c.todo}<span class="badge-soft">{t('spaces.todo.n', { n: c.todo })}</span>{/if}
                {#if c.active}<span class="badge-soft on">{t('spaces.active.n', { n: c.active })}</span>{/if}
                <span class="faint">{t('spaces.made', { a: c.accounts, p: c.projects })}</span>
              </span>
            </button>
          </li>
        {/each}
      </ol>
      {#if work && !work.github}<p class="note">{t('work.github.off')}</p>{/if}
      {#if work?.manage}
        {#if open === 'space'}
          <h2 class="group-title">{t('spaces.add')}</h2>
          <p class="note">{t('work.add_space.note')}</p>
          <SpaceForm spaces={work.workspaces} onfiled={(n) => filed('space', n)} />
          <div class="chooser"><button type="button" class="choose lowers" onclick={() => (open = '')}>{t('accounts.cancel')}</button></div>
        {:else}
          <div class="chooser"><button type="button" class="choose primary" onclick={() => { open = 'space'; notice = ''; }}>+ {t('spaces.add')}</button></div>
        {/if}
      {/if}
    {/if}

  {:else}
    <button type="button" class="back" onclick={() => go('/spaces')}>← {t('spaces.all')}</button>
    {#if !home}{#if !error}<Empty text={t('loading')} />{/if}
    {:else}
      <h2 class="space-head">{titled(home.name, home.title)}</h2>
      <div class="segments space-tabs" role="tablist">
        {#each TABS as k (k)}
          <button type="button" role="tab" class="segment" class:is-on={tab === k} aria-selected={tab === k} onclick={() => goTab(k)}>
            {t('spaces.tab.' + (k || 'overview'))}
            {#if k === 'todo' && home.todo.length}<span class="segment-count">{home.todo.length}</span>{/if}
            {#if k === 'work' && home.active.length}<span class="segment-count">{home.active.length}</span>{/if}
          </button>
        {/each}
      </div>

      {#if tab === ''}
        <div class="space-stats">
          {#each stats as st (st.key)}
            <button type="button" class="space-stat" class:zero={st.n === 0} class:hot={st.hot && st.n > 0} onclick={st.go}>
              <span class="space-stat-n">{st.n}</span>
              <span class="space-stat-label">{t('spaces.stat.' + st.key)}</span>
            </button>
          {/each}
        </div>

        <h3 class="group-title">{t('spaces.needs')}</h3>
        {#if !needs.length}<p class="note">{t('spaces.needs.none')}</p>
        {:else}
          <ul class="rows needs">
            {#each needs as n (n.key)}
              <li class="row"><button type="button" class="linkish need" class:bad={n.bad} onclick={n.go}>{n.text} →</button></li>
            {/each}
          </ul>
        {/if}

        {#if home.active.length}
          <h3 class="group-title">{t('spaces.active')}</h3>
          <ol class="rows">{#each home.active as d (d.id)}{@render activeRow(d)}{/each}</ol>
        {/if}

        {#if home.accounts.length}
          <h3 class="group-title">{t('spaces.todo')}</h3>
          {#if !home.todo.length}<p class="note">{t('spaces.todo.none')}</p>
          {:else}
            <ol class="rows">{#each home.todo.slice(0, 3) as p (p.id)}{@render todoRow(p)}{/each}</ol>
            {#if home.todo.length > 3}<button type="button" class="linkish more-link" onclick={() => goTab('todo')}>{t('spaces.more', { n: home.todo.length })} →</button>{/if}
          {/if}
        {/if}

        <h3 class="group-title">{t('spaces.recent.done')}</h3>
        {#if !home.record.length}<p class="note">{t('work.empty')}</p>
        {:else}
          <ol class="rows">{#each home.record.slice(0, 3) as d (d.id)}{@render doneRow(d)}{/each}</ol>
          {#if home.record.length > 3}<button type="button" class="linkish more-link" onclick={() => goTab('work')}>{t('spaces.more', { n: home.record.length })} →</button>{/if}
        {/if}

      {:else if tab === 'todo'}
        {#if !home.accounts.length}<p class="note">{t('work.spaces.noaccounts')}</p>
        {:else if !home.todo.length}<p class="note">{t('spaces.todo.none')}</p>
        {:else}
          {#if !home.projects.length}<p class="note">{t('spaces.needs.noproject')}</p>{/if}
          <ol class="rows">{#each home.todo as p (p.id)}{@render todoRow(p)}{/each}</ol>
        {/if}

      {:else if tab === 'new'}
        {#if work}
          <WorkForm projects={home.projects} recipes={work.recipes} executors={work.executors} {fromTask} here={'/spaces/' + home.name + '/new'} />
        {/if}

      {:else if tab === 'work'}
        <h3 class="group-title">{t('spaces.active')}</h3>
        {#if !home.active.length}<p class="note">{t('spaces.active.none')}</p>
        {:else}<ol class="rows">{#each home.active as d (d.id)}{@render activeRow(d)}{/each}</ol>{/if}
        <h3 class="group-title">{t('spaces.record')}</h3>
        {#if !home.record.length}<p class="note">{t('work.empty')}</p>
        {:else}<ol class="rows">{#each home.record as d (d.id)}{@render doneRow(d)}{/each}</ol>{/if}

      {:else if tab === 'setup'}
        <div class="space-made">
          <p class="field-label">{t('spaces.accounts')}</p>
          {#if !home.accounts.length}<p class="note">{t('work.spaces.noaccounts')}</p>
          {:else}
            <ul class="rows">
              {#each home.accounts as a (a.key)}
                <li class="row"><span class="who">{a.id}</span> <span class="faint">{t('connector.' + (a.kind === 'mail' ? 'imap' : a.kind))}</span>
                  <span class="account-state {live(a.state)}"> · {a.text}</span></li>
              {/each}
            </ul>
          {/if}
          <p class="field-label">{t('spaces.projects')}</p>
          {#if !home.projects.length}<p class="note">{t('spaces.noprojects')}</p>
          {:else}
            <ul class="rows">
              {#each home.projects as p (p.name)}
                <li class="row"><span class="who">{p.name}</span> <span class="faint">{p.github ?? '—'} · {p.executors.join(', ')}</span></li>
              {/each}
            </ul>
          {/if}
          <p class="field-label">GitHub</p>
          <p class="note">{t('spaces.github.' + home.github)}</p>
        </div>

        {#if home.manage && work}
          {#if open === 'project'}
            <h3 class="group-title">{t('work.add_project')}</h3>
            <p class="note">{t('work.add_project.note')}</p>
            <ProjectForm space={home.name} executors={work.executors} taken={work.projects.map((p) => p.name)} onfiled={(n) => filed('project', n)} />
            <div class="chooser"><button type="button" class="choose lowers" onclick={() => (open = '')}>{t('accounts.cancel')}</button></div>
          {:else if open === 'edit' && thisSpace}
            <h3 class="group-title">{t('spaces.edit')}</h3>
            <p class="note">{t('work.add_space.note')}</p>
            <SpaceForm existing={thisSpace} spaces={work.workspaces} onfiled={(n) => filed('edit', n)} />
            <div class="chooser"><button type="button" class="choose lowers" onclick={() => (open = '')}>{t('accounts.cancel')}</button></div>
          {:else}
            <div class="chooser">
              <button type="button" class="choose primary" onclick={() => { open = 'project'; notice = ''; }}>+ {t('work.add_project')}</button>
              {#if thisSpace}<button type="button" class="choose" onclick={() => { open = 'edit'; notice = ''; }}>{t('spaces.edit')}</button>{/if}
            </div>
          {/if}
        {/if}
      {/if}
    {/if}
  {/if}
</section>

{#snippet todoRow(p: Party)}
  <li class="row todo-row" class:is-on={fromTask === p.id}>
    {#if home?.projects.length}<button type="button" class="choose todo-go" class:is-on={fromTask === p.id} onclick={() => dispatchTask(p.id)}>{t('task.dispatch')}</button>{/if}
    <button type="button" class="linkish" onclick={() => go('/chats/t/' + p.id)}><span class="who">{p.name}</span></button>
    <span class="faint">
      {#if p.due} · {t('chats.due', { day: p.due.slice(5) })}{/if}
      {#if p.from} · {p.from_me ? t('chat.you') : p.from}{/if}
      {#if p.lists?.length} · {p.lists.join(' · ')}{/if}
    </span>
  </li>
{/snippet}

{#snippet activeRow(d: Dispatch)}
  <li class="row todo-row">
    <button type="button" class="choose danger todo-go" disabled={stoppingIds.includes(d.id)} onclick={() => stopWork(d.id)}>{stoppingIds.includes(d.id) ? t('work.stopping') : t('work.stop')}</button>
    <button type="button" class="linkish" onclick={() => go('/work/' + d.id)}><span class="who">{d.project} · {d.recipe}</span></button>
    <span class="faint"> · <span class="state {d.state}">{stateWord(d.state)}</span> · {when(d.started_ms)}</span>
    {#if d.detail}<p class="faint">{d.detail}</p>{/if}
  </li>
{/snippet}

{#snippet doneRow(d: Done)}
  <li class="row">
    <button type="button" class="linkish" onclick={() => go('/work/' + d.id)}><span class="who">{d.project} · {d.recipe}</span></button>
    <span class="faint"> · <span class="state {d.state}">{stateWord(d.state)}</span> · {when(d.started_ms)}</span>
    <p class="faint">{d.sent ? t('spaces.sent', { n: d.sent, to: d.to.join(', ') }) : t('spaces.sent.none')}</p>
  </li>
{/snippet}
