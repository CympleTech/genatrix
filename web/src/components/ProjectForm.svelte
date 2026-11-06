<script lang="ts">
  import { post } from '../lib/api';
  import { t } from '../lib/i18n';

  // Design 06 v0.16, design 13 and 14: a project is added with a form, in
  // the space it belongs to. Which executors may see the code is a box per
  // executor; the commands they may run are one per line. The core writes
  // the file and checks the repository is there.
  let { space, executors, taken, onfiled }: {
    space: string; executors: { name: string; vendor: string }[]; taken: string[]; onfiled: (name: string) => void;
  } = $props();

  let name = $state('');
  let repo = $state('');
  let github = $state('');
  let base = $state('main');
  let chosen = $state<string[]>([]);
  let commands = $state('');
  let network = $state(false);
  let personal = $state(false);
  let more = $state(false);
  let busy = $state(false);
  let error = $state('');

  // Every executor is ticked to start with; untick the ones that may not see it.
  $effect(() => { chosen = executors.map((e) => e.name); });

  // The name follows the repository's folder until typed in by hand.
  let nameTouched = $state(false);
  const slug = (s: string) => s.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 48);
  $effect(() => {
    if (nameTouched) return;
    const fromGithub = github.trim().split('/').pop() ?? '';
    const fromRepo = repo.trim().replace(/\/+$/, '').split('/').pop() ?? '';
    name = slug(fromGithub || fromRepo);
  });
  const nameOk = $derived(/^[a-z0-9][a-z0-9-]{0,47}$/.test(name));
  const githubOk = $derived(!github.trim() || /^[\w.-]+\/[\w.-]+$/.test(github.trim()));
  const isTaken = $derived(taken.includes(name));

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
          github: github.trim() || null, base: base.trim() || 'main',
          executors: chosen,
          commands: commands.split('\n').map((c) => c.trim()).filter(Boolean),
          network, delegate_personal: personal, bundle_cap_mb: 20,
        },
      });
      name = ''; repo = ''; github = ''; base = 'main'; commands = ''; network = false; personal = false; nameTouched = false;
      onfiled(r.name);
    } catch (err: any) { error = err.message; }
    busy = false;
  }
</script>

<form class="setup-form" onsubmit={save}>
  <label>{t('projectform.repo')}
    <input bind:value={repo} placeholder="~/code/my-app" autocapitalize="off" autocomplete="off" spellcheck="false" required />
    <span class="field-note">{t('projectform.repo.note')}</span>
  </label>
  <label>{t('projectform.github')}
    <input bind:value={github} placeholder="me/my-app" autocapitalize="off" autocomplete="off" spellcheck="false" />
    <span class="field-note">{githubOk ? t('projectform.github.note') : t('projectform.github.bad')}</span>
  </label>
  <label>{t('projectform.name')}
    <input bind:value={name} oninput={() => (nameTouched = true)} maxlength="48" autocapitalize="off" autocomplete="off" spellcheck="false" placeholder="my-app" />
    <span class="field-note">{isTaken ? t('projectform.name.taken') : t('spaceform.name.note')}</span>
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
    <label>{t('projectform.base')}
      <input bind:value={base} autocapitalize="off" autocomplete="off" spellcheck="false" />
    </label>
    <label class="quiet-toggle"><input type="checkbox" bind:checked={network} /> {t('projectform.network')}</label>
    <label class="quiet-toggle"><input type="checkbox" bind:checked={personal} /> {t('projectform.personal')}</label>
  {/if}

  {#if error}<p class="note error">{error}</p>{/if}
  <div class="chooser">
    <button type="submit" class="choose primary" disabled={busy || !repo.trim() || !nameOk || isTaken || !githubOk || !chosen.length}>{busy ? t('spaceform.saving') : t('projectform.add')}</button>
  </div>
</form>
