<script lang="ts">
  import { languageChoice, setLanguage, t, type LangChoice } from '../lib/i18n';
  import { framed, openShell } from '../lib/bridge';
  import AccountsPanel from '../components/AccountsPanel.svelte';
  import ModelsPanel from '../components/ModelsPanel.svelte';
  import AgentsPanel from '../components/AgentsPanel.svelte';
  import JunkPanel from '../components/JunkPanel.svelte';
  import SecurityPanel from '../components/SecurityPanel.svelte';
</script>

<section class="pane settings">
  <div class="section">
    <h2 class="group-title">{t('settings.language')}</h2>
    <div class="chooser">
      {#each ['auto', 'zh', 'en'] as const as choice}
        <button type="button" class="choose" class:is-on={languageChoice() === choice}
          onclick={() => { if (languageChoice() !== choice) setLanguage(choice as LangChoice); }}>
          {t(`settings.language.${choice}`)}
        </button>
      {/each}
    </div>
    <p class="note">{t('settings.language.note')}</p>
  </div>
  <AccountsPanel />
  <ModelsPanel />
  <AgentsPanel />
  <JunkPanel />

  <!-- Design 15: devices, pairing, the admin passphrase, export and erase
       are the shell's own panel. Inside the shell's frame this page only
       opens it; outside a frame (development) it shows it here. -->
  {#if framed}
    <div class="section">
      <h2 class="group-title">{t('settings.devices')}</h2>
      <p class="note">{t('shell.security.note')}</p>
      <div class="chooser">
        <button type="button" class="choose primary" onclick={() => openShell('security')}>{t('shell.security.open')}</button>
        <button type="button" class="choose" onclick={() => openShell('interface')}>{t('shell.interface.open')}</button>
      </div>
    </div>
  {:else}
    <SecurityPanel />
  {/if}
</section>
