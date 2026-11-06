<script lang="ts">
  import { get, type Dispatch, type Party, type WorkOverview, type WorkProject } from '../lib/api';
  import { t } from '../lib/i18n';
  import { go, route } from '../lib/router';
  import Empty from '../components/Empty.svelte';
  import WorkForm from '../components/WorkForm.svelte';
  import SpaceForm from '../components/SpaceForm.svelte';
  import ProjectForm from '../components/ProjectForm.svelte';

  // Design 06 v0.14, design 14: a space is where work is done and its
  // record kept. It is a view over the one base, cut by the space's
  // accounts and projects: its tasks are the same conversations Chats
  // shows, its work the same dispatches.
  let { name }: { name: string | null } = $props();

  interface Card { name: string; title: string; accounts: number; projects: number; todo: number; active: number }
  interface AccountLine { key: string; kind: string; id: string; state: string; text: string }
  interface Done extends Dispatch { sent: number; to: string[] }
  interface Home {
    name: string; title: string; accounts: AccountLine[]; projects: WorkProject[];
    todo: Party[]; active: Dispatch[]; record: Done[];
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
  $effect(() => { home = null; open = ''; name; load(); });

  const fromTask = $derived($route.query.get('task'));

  async function filed(what: 'space' | 'project' | 'edit', saved: string) {
    open = ''; error = '';
    notice = t(what === 'project' ? 'projectform.added' : what === 'space' ? 'spaceform.added' : 'work.filed', { name: saved });
    if (what === 'space') { go('/spaces/' + saved); return; }
    await load();
  }
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

      {#if home.accounts.length}
        <h3 class="group-title">{t('spaces.todo')}</h3>
        {#if !home.todo.length}<p class="note">{t('spaces.todo.none')}</p>
        {:else}
          <ol class="rows">
            {#each home.todo as p (p.id)}
              <li class="row">
                <button type="button" class="linkish" onclick={() => go('/chats/t/' + p.id)}><span class="who">{p.name}</span></button>
                <span class="faint">
                  {#if p.due} · {t('chats.due', { day: p.due.slice(5) })}{/if}
                  {#if p.from} · {p.from_me ? t('chat.you') : p.from}{/if}
                  {#if p.lists?.length} · {p.lists.join(' · ')}{/if}
                </span>
              </li>
            {/each}
          </ol>
        {/if}
      {/if}

      {#if home.active.length}
        <h3 class="group-title">{t('spaces.active')}</h3>
        <ol class="rows">
          {#each home.active as d (d.id)}
            <li class="row">
              <button type="button" class="linkish" onclick={() => go('/work/' + d.id)}><span class="who">{d.project} · {d.recipe}</span></button>
              <span class="faint"> · <span class="state {d.state}">{stateWord(d.state)}</span> · {when(d.started_ms)}</span>
              {#if d.detail}<p class="faint">{d.detail}</p>{/if}
            </li>
          {/each}
        </ol>
      {/if}

      <h3 class="group-title">{t('work.new')}</h3>
      {#if work}
        <WorkForm projects={home.projects} recipes={work.recipes} {fromTask} here={'/spaces/' + home.name} />
      {/if}

      <h3 class="group-title">{t('spaces.record')}</h3>
      {#if !home.record.length}<p class="note">{t('work.empty')}</p>
      {:else}
        <ol class="rows">
          {#each home.record as d (d.id)}
            <li class="row">
              <button type="button" class="linkish" onclick={() => go('/work/' + d.id)}><span class="who">{d.project} · {d.recipe}</span></button>
              <span class="faint"> · <span class="state {d.state}">{stateWord(d.state)}</span> · {when(d.started_ms)}</span>
              <p class="faint">{d.sent ? t('spaces.sent', { n: d.sent, to: d.to.join(', ') }) : t('spaces.sent.none')}</p>
            </li>
          {/each}
        </ol>
      {/if}

      <h3 class="group-title">{t('spaces.made.title')}</h3>
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
</section>
