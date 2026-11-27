<script lang="ts">
  import { onMount } from 'svelte';
  import Editor from './lib/Editor.svelte';
  import ProgramList from './lib/ProgramList.svelte';
  import ImportDialog from './lib/ImportDialog.svelte';
  import Settings from './lib/Settings.svelte';
  import { events, updateNotice, type Status } from './lib/api';
  import { applyCssTokens, readHaTokens, watchTheme } from './lib/theme';
  import { loadRegistry, registry } from './lib/registry.svelte';
  import { mayLeave } from './lib/leave';

  function refreshRegistry() {
    loadRegistry().catch((e) => {
      console.warn('registry unavailable', e);
      registry.loaded = true;
    });
  }

  let status = $state<Status | null>(null);
  let route = $state(parse());

  // Hash routing: Ingress serves everything from one path, so real paths would break asset URLs.
  function parse(): { view: 'list' } | { view: 'settings' } | { view: 'program'; id: string } {
    const m = location.hash.match(/^#\/p\/([\w-]+)/);
    if (m) return { view: 'program', id: m[1] };
    return location.hash.startsWith('#/settings') ? { view: 'settings' } : { view: 'list' };
  }

  let importing = $state(false);
  let listKey = $state(0);

  // The hash the current view belongs to; a change to anything else may need confirming.
  let shown = location.hash;

  function go(hash: string) {
    if (hash !== shown && !mayLeave()) return;
    shown = hash;
    location.hash = hash;
  }

  function link(e: MouseEvent) {
    if (e.button !== 0 || e.ctrlKey || e.metaKey || e.shiftKey) return;
    e.preventDefault();
    go(new URL((e.currentTarget as HTMLAnchorElement).href).hash);
  }

  onMount(() => {
    applyCssTokens(readHaTokens());
    const stopTheme = watchTheme(applyCssTokens);
    events.start();
    refreshRegistry();
    const off = events.on((ev) => {
      if (ev.type === 'status' && !('program_id' in ev)) status = ev.status;
      if (ev.type === 'registry_changed') refreshRegistry();
    });
    // Back/forward and typed URLs: if the user stays, put the editor's URL back.
    const onhash = () => {
      if (location.hash !== shown && !mayLeave()) {
        history.pushState(null, '', shown || location.pathname + location.search);
        return;
      }
      shown = location.hash;
      route = parse();
    };
    window.addEventListener('hashchange', onhash);
    return () => {
      off();
      stopTheme();
      window.removeEventListener('hashchange', onhash);
    };
  });

  const notice = $derived(updateNotice(status));
  const label = $derived(
    status === null ? 'App offline' : status.ha_connected ? `HA ${status.ha_version ?? ''}` : 'HA disconnected',
  );
</script>

<header>
  <button class="brand" onclick={() => go('#/')}>
    <svg viewBox="0 0 24 24" width="24" height="24" aria-hidden="true"
      ><path fill="currentColor" d="M20.5 11H19V7a2 2 0 0 0-2-2h-4V3.5a2.5 2.5 0 0 0-5 0V5H4a2 2 0 0 0-2 2v3.8h1.5a2.7 2.7 0 0 1 0 5.4H2V20a2 2 0 0 0 2 2h3.8v-1.5a2.7 2.7 0 0 1 5.4 0V22H17a2 2 0 0 0 2-2v-4h1.5a2.5 2.5 0 0 0 0-5Z"
      /></svg
    >
    <span>Puzzlebox</span>
  </button>
  <nav>
    <a href="#/" class:active={route.view === 'list'} onclick={link}>Programs</a>
    <a href="#/settings" class:active={route.view === 'settings'} onclick={link}>Settings</a>
  </nav>
  <span class="spacer"></span>
  <span class="chip" class:ok={status?.ha_connected} title="Connection to Home Assistant">
    <span class="dot"></span>{label}
  </span>
</header>
{#if notice}
  <div class="notice" class:error={status?.bridge?.compatible === false} role="status">{notice}</div>
{/if}
<main>
  {#if route.view === 'program'}
    {#key route.id}
      <Editor id={route.id} onclose={() => go('#/')} />
    {/key}
  {:else if route.view === 'settings'}
    <Settings />
  {:else}
    {#key listKey}
      <ProgramList onopen={(id) => go(`#/p/${id}`)} onimport={() => (importing = true)} />
    {/key}
  {/if}
</main>
{#if importing}
  <ImportDialog
    onclose={() => (importing = false)}
    ondone={() => {
      importing = false;
      listKey++;
    }}
  />
{/if}

<style>
  header {
    height: 56px;
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 0 16px;
    background: var(--pb-card);
    border-bottom: 1px solid var(--pb-divider);
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: 20px;
    background: none;
    border: 0;
    color: inherit;
    cursor: pointer;
    padding: 0;
  }
  .brand svg {
    color: var(--pb-primary);
  }
  .spacer {
    flex: 1;
  }
  nav {
    display: flex;
    gap: 4px;
    margin-left: 16px;
  }
  nav a {
    color: var(--pb-text-2);
    text-decoration: none;
    padding: 6px 12px;
    border-radius: 16px;
    font-weight: 500;
    font-size: 14px;
  }
  nav a.active {
    color: var(--pb-primary);
    background: color-mix(in srgb, var(--pb-primary) 12%, transparent);
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 4px 12px;
    border-radius: 16px;
    font-size: 13px;
    color: var(--pb-text-2);
    background: color-mix(in srgb, var(--pb-text) 8%, transparent);
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: #db4437;
  }
  .chip.ok .dot {
    background: #43a047;
  }
  .notice {
    flex-shrink: 0;
    padding: 8px 16px;
    font-size: 14px;
    color: var(--pb-text);
    background: color-mix(in srgb, #ffa600 18%, var(--pb-card));
    border-bottom: 1px solid var(--pb-divider);
  }
  .notice.error {
    background: color-mix(in srgb, #db4437 18%, var(--pb-card));
  }
  main {
    position: relative;
    flex: 1;
    display: flex;
    min-height: 0;
  }
</style>
