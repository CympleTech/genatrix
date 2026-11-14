<script lang="ts">
  import { post, type WorkProject } from '../lib/api';
  import { t } from '../lib/i18n';

  // Design 06 v0.16, design 13 and 14: a project is added with a form, in
  // the space it belongs to. Which executors may see the code is a box per
  // executor; the commands they may run are one per line. The core writes
  // the file and checks the repository is there. Changing a project
  // (v0.19) is the same form, filled in, with the name fixed: the change is
  // a new version of the same project, and work already drafted keeps the
  // version it was drafted with.
  let { space, executors, taken, existing = null, onfiled }: {
    space: string; executors: { name: string; vendor: string }[]; taken: string[];
    existing?: WorkProject | null; onfiled: (name: string) => void;
  } = $props();

  // svelte-ignore state_referenced_locally
  const was = existing;
  let name = $state(was?.name ?? '');
  let repo = $state(was?.repo ?? '');
  let github = $state(was?.github ?? '');
  let base = $state(was?.base ?? '');
  let chosen = $state<string[]>(was ? [...was.executors] : []);
  let commands = $state(was?.commands.join('\n') ?? '');
  let network = $state(was?.network ?? false);
  let personal = $state(was?.delegate_personal ?? false);
  let more = $state(!!was && (was.network || was.delegate_personal));
  let busy = $state(false);
  let error = $state('');

  // Every executor is ticked to start with; untick the ones that may not see it.
  $effect(() => { if (!was) chosen = executors.map((e) => e.name); });

  // The name follows the repository's folder until typed in by hand.
  let nameTouched = $state(!!was);
  const slug = (s: string) => s.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 48);
  $effect(() => {
    if (nameTouched) return;
    const fromGithub = github.trim().split('/').pop() ?? '';
    const fromRepo = repo.trim().replace(/\/+$/, '').split('/').pop() ?? '';
    name = slug(fromGithub || fromRepo);
  });
  const nameOk = $derived(/^[a-z0-9][a-z0-9-]{0,47}$/.test(name));
  const githubOk = $derived(!github.trim() || /^[\w.-]+\/[\w.-]+$/.test(github.trim()));
  const isTaken = $derived(!was && taken.includes(name));

  // The base branch is picked from the repository's own local branches:
  // work starts from a local branch, and many repositories have no `main`.
  let branches = $state<string[] | null>(null);
  let current = $state<string | null>(null);
  let repoError = $state('');
  let asked = '';
  let timer: ReturnType<typeof setTimeout> | null = null;
  async function lookUp() {
    const which = repo.trim();
    if (!which || which === asked) return;
    asked = which;
    try {
      const b = await post<{ all: string[]; current: string | null; suggested: string | null }>('/api/work/branches', { repo: which });
      if (repo.trim() !== which) return;
      branches = b.all; current = b.current; repoError = '';
      if (!b.all.includes(base)) base = b.suggested ?? '';
      if (!b.all.length) repoError = t('projectform.branches.none');
    } catch (err: any) {
      if (repo.trim() !== which) return;
      branches = null; current = null; base = ''; repoError = err.message;
    }
  }
  function repoTyped() {
    if (timer) clearTimeout(timer);
    branches = null; repoError = '';
    timer = setTimeout(lookUp, 600);
  }
  $effect(() => () => { if (timer) clearTimeout(timer); });
  // A project being changed has its branches looked up at once.
  if (was) lookUp();

  function toggle(n: string, on: boolean) {
    chosen = on ? [...chosen.filter((x) => x !== n), n] : chosen.filter((x) => x !== n);
  }

  async function save(e: Event) {
    e.preventDefault();
    busy = true; error = '';
    try {
      const r = await post<{ name: string }>('/api/work/define', {
        kind: 'project',
        fields: {
          name, workspace: space, repo: repo.trim(),
          github: github.trim() || null, base,
          executors: chosen,
          commands: commands.split('\n').map((c) => c.trim()).filter(Boolean),
          network, delegate_personal: personal, bundle_cap_mb: was?.bundle_cap_mb ?? 20,
        },
      });
      if (!was) { name = ''; repo = ''; github = ''; base = ''; branches = null; asked = ''; commands = ''; network = false; personal = false; nameTouched = false; }
      onfiled(r.name);
    } catch (err: any) { error = err.message; }
    busy = false;
  }
</script>

<form class="setup-form" onsubmit={save}>
  <label>{t('projectform.repo')}
    <input bind:value={repo} oninput={repoTyped} onblur={lookUp} placeholder="~/code/my-app" autocapitalize="off" autocomplete="off" spellcheck="false" required />
    {#if repoError}<span class="field-note error">{repoError}</span>{:else}<span class="field-note">{t('projectform.repo.note')}</span>{/if}
  </label>
  {#if branches?.length}
    <label>{t('projectform.base')}
      <select bind:value={base}>
        {#each branches as b (b)}<option value={b}>{b}{b === current ? ` · ${t('projectform.base.current')}` : ''}</option>{/each}
      </select>
      <span class="field-note">{t('projectform.base.note')}</span>
    </label>
  {/if}
  <label>{t('projectform.github')}
    <input bind:value={github} placeholder="me/my-app" autocapitalize="off" autocomplete="off" spellcheck="false" />
    <span class="field-note">{githubOk ? t('projectform.github.note') : t('projectform.github.bad')}</span>
  </label>
  <label>{t('projectform.name')}
    <input bind:value={name} oninput={() => (nameTouched = true)} readonly={!!was} maxlength="48" autocapitalize="off" autocomplete="off" spellcheck="false" placeholder="my-app" />
    <span class="field-note">{was ? t('projectform.name.fixed') : isTaken ? t('projectform.name.taken') : t('spaceform.name.note')}</span>
  </label>

  <div class="field">
    <span class="field-label">{t('projectform.executors')}</span>
    {#if !executors.length}<p class="field-note">{t('projectform.executors.none')}</p>
    {:else}
      <ul class="pick-list">
        {#each executors as x (x.name)}
          <li><label>
            <input type="checkbox" checked={chosen.includes(x.name)} onchange={(e) => toggle(x.name, e.currentTarget.checked)} />
            <span class="pick-body"><span class="pick-name">{x.name}</span><span class="faint">{x.vendor}</span></span>
          </label></li>
        {/each}
      </ul>
    {/if}
    <span class="field-note">{t('projectform.executors.note')}</span>
  </div>

  <label>{t('projectform.commands')}
    <textarea bind:value={commands} rows="3" placeholder={'npm test\ncargo test'} spellcheck="false"></textarea>
    <span class="field-note">{t('projectform.commands.note')}</span>
  </label>

  <button type="button" class="linkish" onclick={() => (more = !more)}>{more ? t('projectform.less') : t('projectform.more')}</button>
  {#if more}
    <label class="quiet-toggle"><input type="checkbox" bind:checked={network} /> {t('projectform.network')}</label>
    <label class="quiet-toggle"><input type="checkbox" bind:checked={personal} /> {t('projectform.personal')}</label>
  {/if}

  {#if error}<p class="note error">{error}</p>{/if}
  <div class="chooser">
    <button type="submit" class="choose primary" disabled={busy || !repo.trim() || !base || !nameOk || isTaken || !githubOk || !chosen.length}>{busy ? t('spaceform.saving') : was ? t('projectform.save') : t('projectform.add')}</button>
  </div>
</form>
