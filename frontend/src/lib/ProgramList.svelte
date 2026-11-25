<script lang="ts">
  import { onMount } from 'svelte';
  import { api, events, type Program } from './api';
  import StatusChip from './StatusChip.svelte';

  let { onopen, onimport }: { onopen: (id: string) => void; onimport?: () => void } = $props();

  let programs = $state<Program[]>([]);
  let error = $state<string | null>(null);
  let filter = $state('');

  async function refresh() {
    try {
      programs = await api.programs();
      error = null;
    } catch (e) {
      error = String(e);
    }
  }

  async function create() {
    const p = await api.create({ name: 'New program' });
    onopen(p.id);
  }

  async function toggle(p: Program) {
    const updated = await api.setEnabled(p.id, !p.enabled);
    programs = programs.map((x) => (x.id === p.id ? { ...x, ...updated } : x));
  }

  async function run(p: Program) {
    try {
      await api.run(p.id);
    } catch (e) {
      error = String(e);
    }
  }

  const visible = $derived(
    programs.filter((p) => !filter || p.name.toLowerCase().includes(filter.toLowerCase())),
  );

  function ago(ms: number | null | undefined): string {
    if (!ms) return 'never';
    const s = Math.round((Date.now() - ms) / 1000);
    if (s < 60) return `${s}s ago`;
    if (s < 3600) return `${Math.round(s / 60)}m ago`;
    if (s < 86400) return `${Math.round(s / 3600)}h ago`;
    return new Date(ms).toLocaleDateString();
  }

  onMount(() => {
    refresh();
    return events.on((ev) => {
      if (ev.type === 'status' && 'program_id' in ev) {
        programs = programs.map((p) => (p.id === ev.program_id ? { ...p, status: ev.status } : p));
        if (!programs.some((p) => p.id === ev.program_id)) refresh();
      } else if (ev.type === 'removed') {
        programs = programs.filter((p) => p.id !== ev.program_id);
      }
    });
  });
</script>

<div class="page">
  <div class="toolbar">
    <input class="search" placeholder="Search programs" bind:value={filter} />
    <span class="spacer"></span>
    {#if onimport}<button class="btn" onclick={onimport}>Import</button>{/if}
    <button class="btn primary" onclick={create}>New program</button>
  </div>

  {#if error}<p class="error">{error}</p>{/if}

  {#if programs.length === 0 && !error}
    <div class="empty">
      <h2>No programs yet</h2>
      <p>Programs are automations built from blocks. Create one to get started.</p>
      <button class="btn primary" onclick={create}>Create your first program</button>
    </div>
  {/if}

  <ul class="list">
    {#each visible as p (p.id)}
      <li class="card">
        <button class="main" onclick={() => onopen(p.id)}>
          <span class="name">{p.name}</span>
          <span class="meta">
            {p.mode} · last run {ago(p.status?.last_run)}
            {#if p.status?.last_error}<span class="err"> · {p.status.last_error}</span>{/if}
          </span>
        </button>
        <StatusChip status={p.status} />
        <button class="icon" title="Run now" disabled={!p.enabled} onclick={() => run(p)} aria-label="Run now">▶</button>
        <label class="switch" title={p.enabled ? 'Disable' : 'Enable'}>
          <input type="checkbox" checked={p.enabled} onchange={() => toggle(p)} />
          <span></span>
        </label>
      </li>
    {/each}
  </ul>
</div>

<style>
  .page {
    max-width: 960px;
    margin: 0 auto;
    padding: 16px;
    width: 100%;
    box-sizing: border-box;
    overflow: auto;
  }
  .toolbar {
    display: flex;
    gap: 8px;
    align-items: center;
    margin-bottom: 16px;
    flex-wrap: wrap;
  }
  .spacer {
    flex: 1;
  }
  .search {
    min-width: 200px;
    flex: 1;
    max-width: 360px;
  }
  .list {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    gap: 8px;
  }
  .card {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 4px 12px 4px 4px;
    background: var(--pb-card);
    border: 1px solid var(--pb-divider);
    border-radius: 12px;
  }
  .main {
    flex: 1;
    min-width: 0;
    text-align: left;
    background: none;
    border: 0;
    padding: 12px;
    color: inherit;
    cursor: pointer;
    display: flex;
    flex-direction: column;
    gap: 2px;
    border-radius: 8px;
  }
  .main:hover {
    background: color-mix(in srgb, var(--pb-text) 5%, transparent);
  }
  .name {
    font-size: 16px;
    font-weight: 500;
  }
  .meta {
    font-size: 13px;
    color: var(--pb-text-2);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .err {
    color: #e53935;
  }
  .empty {
    text-align: center;
    padding: 64px 16px;
    color: var(--pb-text-2);
  }
  .empty h2 {
    color: var(--pb-text);
    font-weight: 400;
  }
  .error {
    color: #e53935;
  }
</style>
