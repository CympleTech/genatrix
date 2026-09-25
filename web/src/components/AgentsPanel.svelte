<script lang="ts">
  import { get, type AgentCard, type AgentPreview, type Risk } from '../lib/api';
  import { t } from '../lib/i18n';
  import { bytes, cardValue } from '../lib/format';
  import { go } from '../lib/router';
  import Icon from './Icon.svelte';

  // Design 11, "安装" and "审查与管理": what a package may do, in words,
  // before anything of it runs for real; a trial on recent items; then the
  // user's word. Installing and uninstalling happen on this machine only
  // (design 02, invariant 18); a paired phone sees the list and can pause.
  let agents = $state<AgentCard[] | null>(null);
  let local = $state(false);
  let error = $state('');
  let notice = $state('');
  let preview = $state<AgentPreview | null>(null);
  let reading = $state(false);
  let installing = $state(false);
  let confirming = $state('');
  let file = $state<HTMLInputElement | null>(null);

  async function load() {
    try {
      agents = await get<AgentCard[]>('/api/agents');
      local = (await get<{ local: boolean }>('/api/setup')).local;
    } catch (e: any) { error = e.message; }
  }
  $effect(() => { load(); });

  async function chosen() {
    const f = file?.files?.[0];
    if (!f) return;
    reading = true; error = ''; notice = ''; preview = null;
    try {
      const r = await fetch('/api/agents/preview', {
        method: 'POST', credentials: 'same-origin',
        headers: { 'content-type': 'application/wasm' }, body: await f.arrayBuffer(),
      });
      const body = await r.json().catch(() => null);
      if (!r.ok) throw new Error(body?.error || r.statusText);
      preview = body;
    } catch (e: any) { error = e.message; }
    reading = false;
    if (file) file.value = '';
  }
  async function install() {
    if (!preview) return;
    installing = true; error = '';
    try {
      const r = await fetch('/api/agents/install', {
        method: 'POST', credentials: 'same-origin',
        headers: { 'content-type': 'application/json' }, body: JSON.stringify({ hash: preview.hash }),
      });
      const body = await r.json().catch(() => null);
      if (!r.ok) throw new Error(body?.error || r.statusText);
      notice = t('agents.installed', { name: preview.name });
      preview = null;
      await load();
    } catch (e: any) { error = e.message; }
    installing = false;
  }
  async function act(a: AgentCard, what: 'pause' | 'resume' | 'uninstall') {
    if (what === 'uninstall' && confirming !== a.id) { confirming = a.id; return; }
    confirming = '';
    try {
      const r = await fetch(`/api/agent/${a.id}/${what}`, { method: 'POST', credentials: 'same-origin', headers: { 'content-type': 'application/json' }, body: '{}' });
      const body = await r.json().catch(() => null);
      if (!r.ok) throw new Error(body?.error || r.statusText);
      if (what === 'uninstall') notice = t('agents.removed', { name: a.name });
      if (what === 'pause' && body?.withdrawn) notice = t('agents.withdrawn', { n: body.withdrawn });
      await load();
    } catch (e: any) { error = e.message; }
  }
  const riskWord = (r: Risk) => t(`agents.risk.${r}`);
</script>

<div class="section agents-panel">
  <h2 class="group-title">{t('agents.title')}</h2>
  <p class="note">{t('agents.note')}</p>
  {#if error}<p class="note error">{error}</p>{/if}
  {#if notice}<p class="note ok-note">{notice}</p>{/if}

  {#if agents && !agents.length && !preview}<p class="note">{t('agents.none')}</p>{/if}
  <ul class="account-list">
    {#each agents ?? [] as a (a.id)}
      <li class="account-row">
        <span class="account-icon"><Icon name="agent" size={18} /></span>
        <span class="account-body">
          <button type="button" class="linkish account-name" onclick={() => go(`/chats/a/${a.id}`)}>{a.name}</button>
          <span class="account-state" class:bad={a.state === 'paused'}>
            {a.state === 'paused' ? t('agents.paused') : t('agents.active')} · <span class="risk {a.risk}">{riskWord(a.risk)}</span> · {bytes(a.space_bytes)} · {t('agents.runs', { n: a.runs })}
          </span>
        </span>
        {#if a.state === 'active'}
          <button type="button" class="choose lowers" onclick={() => act(a, 'pause')}>{t('agents.pause')}</button>
        {:else}
          <button type="button" class="choose" onclick={() => act(a, 'resume')}>{t('agents.resume')}</button>
        {/if}
        {#if local}
          <a class="choose" href={`/api/agent/${a.id}/export`} download>{t('agents.export')}</a>
          <button type="button" class="choose lowers" onclick={() => act(a, 'uninstall')}>
            {confirming === a.id ? t('agents.confirmRemove') : t('agents.remove')}
          </button>
        {/if}
      </li>
    {/each}
  </ul>
  {#if confirming}<p class="note">{t('agents.removeNote')}</p>{/if}

  {#if !local}
    <p class="note">{t('agents.onlyHere')}</p>
  {:else if preview}
    <!-- What the package may do, read from its manifest without running it,
         then what it did on recent items with nothing kept. -->
    <div class="agent-preview">
      <div class="preview-head">
        <span class="account-icon"><Icon name="agent" size={18} /></span>
        <span class="account-body">
          <span class="account-name">{preview.name}</span>
          <span class="account-state">{t('agents.by', { author: preview.author })} · {t('agents.version', { v: preview.hash.slice(0, 12) })}</span>
        </span>
        <span class="risk-badge {preview.risk}">{riskWord(preview.risk)}</span>
      </div>
      <p class="preview-purpose">{preview.purpose}</p>
      <dl class="card-fields grants">
        {#each preview.lines as l}<dt>{t(`agents.grant.${l.label}`)}</dt><dd>{l.text}</dd>{/each}
      </dl>

      <h4 class="card-label">{t('agents.trial')}</h4>
      {#if preview.trial.outcome === 'none'}
        <p class="note">{t('agents.trial.none')}</p>
      {:else}
        <p class="note">
          {t('agents.trial.items', { n: preview.trial.items })}
          {#if preview.trial.outcome !== 'ok'} · <span class="error">{preview.trial.outcome}: {preview.trial.detail}</span>{/if}
        </p>
        {#if preview.trial.proposals.length}
          <ul class="tried">
            {#each preview.trial.proposals as p}
              <li>
                <span class="tried-head"><b>{p.label}</b> → {p.reach}</span>
                {#if p.card}
                  <dl class="card-fields">
                    {#each p.card.fields as f}<dt>{f.label}</dt><dd>{cardValue(f.value)}</dd>{/each}
                  </dl>
                {:else if p.draft}<p class="tried-draft">{p.draft}</p>{/if}
              </li>
            {/each}
          </ul>
        {:else}<p class="note">{t('agents.trial.noproposals')}</p>{/if}
        {#if preview.trial.log.length}
          <details class="trial-log"><summary>{t('agents.trial.log', { n: preview.trial.log.length })}</summary>
            <pre>{preview.trial.log.slice(0, 50).join('\n')}</pre>
          </details>
        {/if}
      {/if}
      <div class="chooser">
        <button type="button" class="choose primary" disabled={installing} onclick={install}>{t('agents.install')}</button>
        <button type="button" class="choose lowers" onclick={() => (preview = null)}>{t('agents.cancel')}</button>
      </div>
    </div>
  {:else}
    <div class="chooser">
      <label class="choose">
        <Icon name="upload" size={16} />{reading ? t('agents.reading') : t('agents.choose')}
        <input bind:this={file} type="file" accept=".wasm" class="hidden-file" onchange={chosen} disabled={reading} />
      </label>
    </div>
  {/if}
</div>
