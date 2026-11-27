<script lang="ts">
  import { onMount } from 'svelte';
  import { api, events, type Program } from './api';
  import StatusChip from './StatusChip.svelte';

  let { onopen, onimport }: { onopen: (id: string) => void; onimport?: () => void } = $props();

  let programs = $state<Program[]>([]);
  let error = $state<string | null>(null);
  let filter = $state('');
  let moving = $state<Program | null>(null);
  let moveTo = $state('');
  let dragging = $state<string | null>(null);
  let dropTarget = $state<string | null>(null);

  const CLOSED_KEY = 'puzzlebox.closedFolders';
  let closed = $state<Set<string>>(loadClosed());
  function loadClosed(): Set<string> {
    try {
      return new Set(JSON.parse(localStorage.getItem(CLOSED_KEY) ?? '[]'));
    } catch {
      return new Set();
    }
  }
  function toggleFolder(path: string) {
    const next = new Set(closed);
    if (next.has(path)) next.delete(path);
    else next.add(path);
    closed = next;
    try {
      localStorage.setItem(CLOSED_KEY, JSON.stringify([...next]));
    } catch {
      /* remembered for this page only */
    }
  }

  interface Folder {
    name: string;
    path: string;
    folders: Folder[];
    programs: Program[];
    count: number;
  }

  function buildTree(list: Program[]): Folder {
    const root: Folder = { name: '', path: '', folders: [], programs: [], count: 0 };
    for (const p of list) {
      let node = root;
      node.count++;
      let path = '';
      for (const part of (p.folder ?? '').split('/').filter(Boolean)) {
        path = path ? `${path}/${part}` : part;
        let child = node.folders.find((f) => f.name === part);
        if (!child) {
          child = { name: part, path, folders: [], programs: [], count: 0 };
          node.folders.push(child);
        }
        child.count++;
        node = child;
      }
      node.programs.push(p);
    }
    const sort = (f: Folder) => {
      f.folders.sort((a, b) => a.name.localeCompare(b.name));
      f.folders.forEach(sort);
    };
    sort(root);
    return root;
  }

  const allFolders = $derived([...new Set(programs.map((p) => p.folder).filter(Boolean))].sort());

  async function setFolder(p: Program, folder: string) {
    try {
      const updated = await api.update(p.id, { folder });
      programs = programs.map((x) => (x.id === p.id ? { ...x, folder: updated.folder } : x));
    } catch (e) {
      error = String(e);
    }
  }

  function startMove(p: Program) {
    moving = p;
    moveTo = p.folder ?? '';
  }

  async function confirmMove() {
    if (moving) await setFolder(moving, moveTo);
    moving = null;
  }

  function onDrop(path: string) {
    const p = programs.find((x) => x.id === dragging);
    dragging = dropTarget = null;
    if (p && (p.folder ?? '') !== path) setFolder(p, path);
  }

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

  // Every word must appear in the name, folder or description.
  const visible = $derived.by(() => {
    const words = filter.toLowerCase().split(/\s+/).filter(Boolean);
    if (!words.length) return programs;
    return programs.filter((p) => {
      const hay = `${p.name} ${p.folder ?? ''} ${p.description ?? ''}`.toLowerCase();
      return words.every((w) => hay.includes(w));
    });
  });
  const tree = $derived(buildTree(visible));
  const searching = $derived(filter.trim().length > 0);

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

<svelte:window onkeydown={(e) => moving && e.key === 'Escape' && (moving = null)} />

<div class="page">
  <div class="toolbar">
    <input class="search" placeholder="Search programs and folders" bind:value={filter} />
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

  {#snippet card(p: Program)}
    <li
      class="card"
      draggable="true"
      ondragstart={(e) => {
        dragging = p.id;
        e.dataTransfer?.setData('text/plain', p.id);
      }}
      ondragend={() => (dragging = dropTarget = null)}
    >
      <button class="main" onclick={() => onopen(p.id)}>
        <span class="name">{p.name}</span>
        <span class="meta">
          {#if searching && p.folder}<span class="folder-tag">{p.folder.replaceAll('/', ' › ')}</span> · {/if}
          {p.mode} · last run {ago(p.status?.last_run)}
          {#if p.status?.last_error}<span class="err"> · {p.status.last_error}</span>{/if}
        </span>
      </button>
      <StatusChip status={p.status} />
      <button class="icon" title="Move to folder" onclick={() => startMove(p)} aria-label="Move to folder">⇄</button>
      <button class="icon" title="Run now" disabled={!p.enabled} onclick={() => run(p)} aria-label="Run now">▶</button>
      <label class="switch" title={p.enabled ? 'Disable' : 'Enable'}>
        <input type="checkbox" checked={p.enabled} onchange={() => toggle(p)} />
        <span></span>
      </label>
    </li>
  {/snippet}

  {#snippet folderView(f: Folder)}
    {#each f.folders as sub (sub.path)}
      {@const open = searching || !closed.has(sub.path)}
      <li class="folder">
        <button
          class="folder-head"
          class:drop={dropTarget === sub.path}
          onclick={() => toggleFolder(sub.path)}
          ondragover={(e) => {
            if (!dragging) return;
            e.preventDefault();
            dropTarget = sub.path;
          }}
          ondragleave={() => dropTarget === sub.path && (dropTarget = null)}
          ondrop={(e) => {
            e.preventDefault();
            onDrop(sub.path);
          }}
          aria-expanded={open}
        >
          <span class="chev" class:open>▸</span>
          <span class="folder-name">{sub.name}</span>
          <span class="count">{sub.count}</span>
        </button>
        {#if open}
          <ul class="list">
            {@render folderView(sub)}
          </ul>
        {/if}
      </li>
    {/each}
    {#each f.programs as p (p.id)}
      {@render card(p)}
    {/each}
  {/snippet}

  {#if dragging && programs.find((p) => p.id === dragging)?.folder}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="unfile"
      class:drop={dropTarget === ''}
      ondragover={(e) => {
        e.preventDefault();
        dropTarget = '';
      }}
      ondrop={(e) => {
        e.preventDefault();
        onDrop('');
      }}
    >
      Drop here to take it out of its folder
    </div>
  {/if}

  <ul class="list">
    {@render folderView(tree)}
  </ul>

  {#if moving}
    <div class="dialog-backdrop" role="presentation" onclick={() => (moving = null)}></div>
    <form class="dialog" onsubmit={(e) => (e.preventDefault(), confirmMove())}>
      <h3>Move “{moving.name}”</h3>
      <label>
        Folder
        <input list="pb-folders" bind:value={moveTo} placeholder="e.g. Lights/Outside (empty: no folder)" />
      </label>
      <datalist id="pb-folders">
        {#each allFolders as f (f)}<option value={f}></option>{/each}
      </datalist>
      <p class="muted">Use “/” for folders inside folders. You can also drag programs onto a folder.</p>
      <div class="actions">
        <button type="button" class="btn" onclick={() => (moving = null)}>Cancel</button>
        <button type="submit" class="btn primary">Move</button>
      </div>
    </form>
  {/if}
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
  @media (max-width: 560px) {
    .page {
      padding: 12px;
    }
    .search {
      min-width: 0;
      max-width: none;
      flex-basis: 100%;
    }
    .list {
      grid-template-columns: minmax(0, 1fr);
    }
    .card {
      flex-wrap: wrap;
      gap: 4px 8px;
      padding: 4px 8px 8px 4px;
    }
    .main {
      flex-basis: 100%;
      padding: 10px 12px 4px;
    }
    .card > :global(.chip) {
      margin: 0 auto 0 12px;
    }
  }
  .folder {
    list-style: none;
  }
  .folder > .list {
    margin: 8px 0 0 16px;
  }
  .folder-head {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 8px 12px;
    border: 1px dashed transparent;
    border-radius: 10px;
    background: color-mix(in srgb, var(--pb-text) 4%, transparent);
    color: inherit;
    font: inherit;
    font-weight: 500;
    cursor: pointer;
    text-align: left;
  }
  .folder-head.drop,
  .unfile.drop {
    border-color: var(--pb-primary);
    background: color-mix(in srgb, var(--pb-primary) 12%, transparent);
  }
  .chev {
    display: inline-block;
    transition: transform 0.15s;
    color: var(--pb-text-2);
  }
  .chev.open {
    transform: rotate(90deg);
  }
  .folder-name {
    flex: 1;
  }
  .count,
  .folder-tag {
    color: var(--pb-text-2);
    font-size: 13px;
    font-weight: 400;
  }
  .unfile {
    padding: 12px;
    margin-bottom: 8px;
    border: 1px dashed var(--pb-divider);
    border-radius: 10px;
    text-align: center;
    color: var(--pb-text-2);
  }
  .dialog-backdrop {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 30%);
    z-index: 40;
  }
  .dialog {
    position: fixed;
    z-index: 41;
    left: 50%;
    top: 30%;
    transform: translate(-50%, -30%);
    width: min(420px, calc(100% - 32px));
    padding: 16px 20px;
    background: var(--pb-card);
    border-radius: 12px;
    box-shadow: 0 8px 28px rgb(0 0 0 / 25%);
  }
  .dialog h3 {
    margin-top: 0;
    font-weight: 500;
  }
  .dialog label {
    display: grid;
    gap: 4px;
  }
  .muted {
    color: var(--pb-text-2);
    font-size: 13px;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
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
