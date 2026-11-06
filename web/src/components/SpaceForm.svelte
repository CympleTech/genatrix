<script lang="ts">
  import { get, post, type Workspace } from '../lib/api';
  import { t } from '../lib/i18n';

  // Design 06 v0.16, design 14: a space is added and changed with a form.
  // Its accounts are picked from the ones connected in Settings; an account
  // already in another space is shown but cannot be picked (invariant 30).
  // The core writes the file; nobody types TOML on the page.
  let { existing = null, spaces, onfiled }: {
    existing?: Workspace | null; spaces: Workspace[]; onfiled: (name: string) => void;
  } = $props();

  interface Connected { kind: string; id: string; name: string }
  let connected = $state<Connected[] | null>(null);
  let title = $state('');
  let name = $state('');
  let nameTouched = $state(false);
  let chosen = $state<string[]>([]);
  let busy = $state(false);
  let error = $state('');

  $effect(() => {
    title = existing?.title ?? '';
    name = existing?.name ?? '';
    chosen = [...(existing?.accounts ?? [])];
    nameTouched = false;
  });
  $effect(() => {
    get<{ accounts: Connected[] }>('/api/setup')
      .then((s) => (connected = s.accounts))
      .catch((e: any) => { error = e.message; connected = []; });
  });

  // The short name goes in addresses and files: lowercase letters, digits
  // and dashes. It follows the title until it is typed in by hand.
  const slug = (s: string) => s.toLowerCase().normalize('NFKD').replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 48);
  $effect(() => { if (!existing && !nameTouched) name = slug(title); });
  const nameOk = $derived(/^[a-z0-9][a-z0-9-]{0,47}$/.test(name));
  const taken = $derived(!existing && spaces.some((s) => s.name === name));

  const key = (a: Connected) => `${a.kind}:${a.id}`;
  const ownerOf = (k: string) => spaces.find((s) => s.name !== existing?.name && s.accounts.includes(k));
  const spaceWord = (s: Workspace) => (s.name === 'personal' && (!s.title || s.title === 'Personal') ? t('spaces.personal') : s.title || s.name);
  // Accounts the space names that are not connected here any more stay on
  // the list, so saving does not drop them without being asked.
  const gone = $derived(chosen.filter((k) => connected && !connected.some((a) => key(a) === k)));

  function toggle(k: string, on: boolean) {
    chosen = on ? [...chosen.filter((x) => x !== k), k] : chosen.filter((x) => x !== k);
  }

  async function save(e: Event) {
    e.preventDefault();
    busy = true; error = '';
    try {
      const r = await post<{ name: string }>('/api/work/define', {
        kind: 'workspace',
        fields: { name, title: title.trim(), accounts: chosen },
      });
      onfiled(r.name);
    } catch (err: any) { error = err.message; }
    busy = false;
  }
</script>

<form class="setup-form" onsubmit={save}>
  <label>{t('spaceform.title')}
    <input bind:value={title} maxlength="80" placeholder={t('spaceform.title.eg')} required />
  </label>
  {#if !existing}
    <label>{t('spaceform.name')}
      <input bind:value={name} oninput={() => (nameTouched = true)} maxlength="48" autocapitalize="off" autocomplete="off" spellcheck="false" placeholder="company" />
      <span class="field-note">{taken ? t('spaceform.name.taken') : t('spaceform.name.note')}</span>
    </label>
  {/if}

  <div class="field">
    <span class="field-label">{t('spaceform.accounts')}</span>
    {#if !connected}<p class="field-note">{t('loading')}</p>
    {:else if !connected.length && !gone.length}<p class="field-note">{t('spaceform.accounts.none')}</p>
    {:else}
      <ul class="pick-list">
        {#each connected as a (key(a))}
          {@const k = key(a)}
          {@const owner = ownerOf(k)}
          <li>
            <label class:off={!!owner}>
              <input type="checkbox" checked={chosen.includes(k)} disabled={!!owner} onchange={(e) => toggle(k, e.currentTarget.checked)} />
              <span class="pick-body">
                <span class="pick-name">{a.name}</span>
                <span class="faint">{t('connector.' + (a.kind === 'mail' ? 'imap' : a.kind))}{#if owner} · {t('spaceform.inother', { space: spaceWord(owner) })}{/if}</span>
              </span>
            </label>
          </li>
        {/each}
        {#each gone as k (k)}
          <li>
            <label>
              <input type="checkbox" checked onchange={(e) => toggle(k, e.currentTarget.checked)} />
              <span class="pick-body"><span class="pick-name">{k.split(':').slice(1).join(':')}</span><span class="faint">{t('spaceform.gone')}</span></span>
            </label>
          </li>
        {/each}
      </ul>
    {/if}
    <span class="field-note">{t('spaceform.accounts.note')}</span>
  </div>

  {#if error}<p class="note error">{error}</p>{/if}
  <div class="chooser">
    <button type="submit" class="choose primary" disabled={busy || !title.trim() || !nameOk || taken}>{busy ? t('spaceform.saving') : existing ? t('spaceform.save') : t('spaceform.add')}</button>
  </div>
</form>
