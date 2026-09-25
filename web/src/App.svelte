<script lang="ts">
  import { route, go } from './lib/router';
  import { t } from './lib/i18n';
  import { status, pending, unauthorized, statusError, startPolling } from './lib/status';
  import Today from './pages/Today.svelte';
  import Timeline from './pages/Timeline.svelte';
  import Ask from './pages/Ask.svelte';
  import Approvals from './pages/Approvals.svelte';
  import Chats from './pages/Chats.svelte';
  import Review from './pages/Review.svelte';
  import Records from './pages/Records.svelte';
  import Settings from './pages/Settings.svelte';
  import Pair from './pages/Pair.svelte';
  import Welcome from './pages/Welcome.svelte';
  import { get as fetchJson } from './lib/api';
  import Icon from './components/Icon.svelte';

  startPolling();

  // Design 09: with no account yet, on the machine itself, the page opens on
  // the wizard rather than on an empty Today. Skipping it is remembered.
  (async () => {
    try {
      if (localStorage.getItem('genatrix.welcome.skipped')) return;
    } catch { /* no storage: ask every time, which is harmless */ }
    try {
      const setup = await fetchJson<{ local: boolean; accounts: unknown[] }>('/api/setup');
      if (setup.local && setup.accounts.length === 0 && ($route.parts[0] ?? '') === '') go('/welcome', true);
    } catch { /* the core is still starting; the next visit will ask */ }
  })();

  // The four faces (design 06) and the approval panel are always one tap
  // away; the rest sits behind "more" on a phone and in the rail on a desktop.
  // Design 06 v0.5: four faces, what the day asks of you, what waits for
  // your word, who has been talking to you, and a question of your own.
  // The archive and the audit are one tap further away, not gone.
  const primary = [
    { path: '/', key: 'today' },
    { path: '/approvals', key: 'approvals' },
    { path: '/chats', key: 'chats' },
    { path: '/ask', key: 'ask' },
  ];
  const secondary = [
    { path: '/timeline', key: 'timeline' },
    { path: '/records', key: 'records' },
    { path: '/review', key: 'review' },
    { path: '/settings', key: 'settings' },
  ];
  // Old links to /people still arrive somewhere sensible.
  $effect(() => {
    if ($route.parts[0] === 'people') go('/chats' + ($route.parts[1] ? '/' + $route.parts[1] : ''), true);
  });
  let more = $state(false);

  const section = $derived($route.parts[0] ?? '');
  const title = $derived.by(() => {
    const all = [...primary, ...secondary].find((n) => (n.path === '/' ? section === '' : section === n.path.slice(1)));
    return all ? t(all.key) : section === 'pair' ? t('pair.title') : section === 'welcome' ? t('welcome.title') : 'Genatrix';
  });
  function on(path: string): boolean {
    return path === '/' ? section === '' : section === path.slice(1);
  }
  function nav(path: string) { more = false; go(path); }

  const statusLine = $derived.by(() => {
    const s = $status;
    if (!s) return $statusError || t('loading');
    const left = s.bytes_left_device === 0 ? t('status.left.none') : t('status.left.some', { n: s.bytes_left_device });
    return [t('status.items', { n: s.items }), t('status.judged', { n: s.judged }), t('status.model', { s: s.model_text }),
      s.cloud_enabled ? t('status.cloud.on') : t('status.cloud.off'), left].join(' · ');
  });
</script>

<div class="shell" class:more-open={more}>
  <header class="top">
    <h1><span class="brand">Genatrix</span><span class="page-title">{title}</span></h1>
    <p class="status" class:error={!!$statusError}>{statusLine}</p>
  </header>

  <nav class="rail" aria-label="sections">
    {#each primary as n}
      <button type="button" class="nav" class:is-on={on(n.path)} onclick={() => nav(n.path)}>
        <Icon name={n.key} /><span>{t(n.key)}</span>{#if n.key === 'approvals' && $pending}<span class="badge">{$pending}</span>{/if}
      </button>
    {/each}
    <span class="rail-gap"></span>
    {#each secondary as n}
      <button type="button" class="nav secondary" class:is-on={on(n.path)} onclick={() => nav(n.path)}><Icon name={n.key} size={18} /><span>{t(n.key)}</span></button>
    {/each}
  </nav>

  <main>
    {#if $unauthorized && section !== 'pair'}
      <section class="pane">
        <p class="headline">{t('unauthorized')}</p>
        <p class="note">{t('unauthorized.hint')}</p>
        <button type="button" class="choose" onclick={() => go('/pair')}>{t('pair.title')}</button>
      </section>
    {:else if section === ''}<Today />
    {:else if section === 'timeline'}<Timeline />
    {:else if section === 'ask'}<Ask />
    {:else if section === 'approvals'}<Approvals />
    {:else if section === 'chats'}
      <!-- One instance for every chats route: a person and a group are
           arguments to the same page, so opening one never rebuilds the list. -->
      <Chats
        kind={$route.parts[1] === 'g' ? 'group' : $route.parts[1] === 'a' ? 'agent' : 'person'}
        id={($route.parts[1] === 'g' || $route.parts[1] === 'a' ? $route.parts[2] : $route.parts[1]) ?? null}
      />
    {:else if section === 'review'}<Review />
    {:else if section === 'records'}<Records />
    {:else if section === 'settings'}<Settings />
    {:else if section === 'pair'}<Pair />
    {:else if section === 'welcome'}<Welcome />
    {:else}<Today />{/if}
  </main>

  <nav class="bar" aria-label="sections">
    {#each primary as n}
      <button type="button" class="nav" class:is-on={on(n.path)} onclick={() => nav(n.path)}>
        <Icon name={n.key} size={22} /><span>{t(n.key)}</span>{#if n.key === 'approvals' && $pending}<span class="badge">{$pending}</span>{/if}
      </button>
    {/each}
    <button type="button" class="nav" class:is-on={more || secondary.some((n) => on(n.path))} onclick={() => (more = !more)}><Icon name="more" size={22} /><span>{t('more')}</span></button>
  </nav>
  {#if more}
    <div class="more-sheet" role="dialog">
      {#each secondary as n}
        <button type="button" class="nav" class:is-on={on(n.path)} onclick={() => nav(n.path)}><Icon name={n.key} /><span>{t(n.key)}</span></button>
      {/each}
    </div>
    <div class="scrim" onclick={() => (more = false)} role="presentation"></div>
  {/if}
</div>
