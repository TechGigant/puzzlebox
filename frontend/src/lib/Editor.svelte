<script lang="ts">
  import { onMount } from 'svelte';
  import BlocklyWorkspace from './BlocklyWorkspace.svelte';
  import StatusChip from './StatusChip.svelte';
  import SidePanel from './SidePanel.svelte';
  import { api, events, type CompileError, type Program, type ProgramVersion, type RunMode } from './api';
  import { registry } from './registry.svelte';

  let { id, onclose }: { id: string; onclose: () => void } = $props();

  let program = $state<Program | null>(null);
  let name = $state('');
  let mode = $state<RunMode>('single');
  let enabled = $state(true);
  let dirty = $state(false);
  let saving = $state(false);
  let message = $state<string | null>(null);
  let errors = $state<CompileError[]>([]);
  let paused = $state<{ run_id: string; block_id: string; variables: Record<string, unknown> } | null>(null);
  let panelOpen = $state(true);
  let ws: BlocklyWorkspace | undefined = $state();
  let current: object = {};
  let baseline = '';
  let meta = '';
  const metaKey = () => JSON.stringify([name, mode, enabled]);
  let compileTimer: ReturnType<typeof setTimeout> | undefined;
  let blockCount = $state(0);
  let history = $state<ProgramVersion[] | null>(null);

  /** Blocks in a saved workspace, not counting shadows (the default values in sockets). */
  function countBlocks(node: unknown): number {
    if (!node || typeof node !== 'object') return 0;
    if (Array.isArray(node)) return node.reduce((n, x) => n + countBlocks(x), 0);
    let n = 0;
    for (const [k, v] of Object.entries(node as Record<string, unknown>)) {
      if (k === 'shadow') continue;
      if (k === 'block') n += 1;
      n += countBlocks(v);
    }
    return n;
  }

  async function openHistory() {
    try {
      history = await api.versions(id);
    } catch (e) {
      flash(String(e));
    }
  }

  async function restore(v: ProgramVersion) {
    const full = await api.version(id, v.id);
    if (!full.workspace) return;
    if (dirty && !confirm('Replace your unsaved changes with this version?')) return;
    ws?.load(full.workspace);
    history = null;
    // Loading doesn't count as an edit; mark the restored blocks as unsaved changes.
    setTimeout(() => ws && onchange(ws.save()), 10);
    flash(`Restored the version from ${new Date(v.saved_at).toLocaleString()}. Save to keep it.`);
  }

  async function load() {
    const p = await api.program(id);
    program = p;
    name = p.name;
    mode = p.mode;
    enabled = p.enabled;
    current = p.workspace ?? {};
    errors = p.status?.errors ?? [];
  }

  function refreshDirty() {
    dirty = JSON.stringify(current) !== baseline || metaKey() !== meta;
  }

  function onchange(state: object) {
    current = state;
    blockCount = countBlocks((state as { blocks?: unknown }).blocks ?? null) + ((state as { blocks?: { blocks?: unknown[] } }).blocks?.blocks?.length ?? 0);
    if (!baseline) return;
    refreshDirty();
    clearTimeout(compileTimer);
    compileTimer = setTimeout(async () => {
      try {
        errors = (await api.compile(current)).errors;
        ws?.setErrors(errors);
      } catch {
        /* offline; save will report */
      }
    }, 500);
  }

  async function save() {
    if (!program) return;
    saving = true;
    try {
      const saved = await api.update(program.id, { name, mode, enabled, workspace: ws?.save() ?? current });
      program = saved;
      baseline = JSON.stringify(ws?.save() ?? current);
      meta = metaKey();
      errors = saved.errors ?? [];
      ws?.setErrors(errors);
      dirty = false;
      flash(errors.length ? `Saved with ${errors.length} problem(s); the program won't run until they're fixed.` : 'Saved');
    } catch (e) {
      flash(`Save failed: ${e}`);
    } finally {
      saving = false;
    }
  }

  async function run() {
    if (dirty) await save();
    try {
      await api.run(id);
    } catch (e) {
      flash(String(e));
    }
  }

  async function runStack(topBlockId: string) {
    if (dirty) await save();
    try {
      await api.runStack(id, topBlockId);
      flash('Running');
    } catch (e) {
      flash(String(e));
    }
  }

  async function remove() {
    if (!confirm(`Delete "${name}"? This cannot be undone.`)) return;
    await api.remove(id);
    onclose();
  }

  let flashTimer: ReturnType<typeof setTimeout> | undefined;
  function flash(m: string) {
    message = m;
    clearTimeout(flashTimer);
    flashTimer = setTimeout(() => (message = null), 4000);
  }

  function onkey(e: KeyboardEvent) {
    if ((e.ctrlKey || e.metaKey) && e.key === 's') {
      e.preventDefault();
      save();
    }
  }

  function setBreakpoints(blocks: string[]) {
    events.send({ type: 'breakpoints', program_id: id, blocks }, `bp:${id}`);
  }

  function resume(step: boolean) {
    if (!paused) return;
    events.send({ type: step ? 'step' : 'resume', run_id: paused.run_id });
  }

  function guardLeave(e: BeforeUnloadEvent) {
    if (dirty) e.preventDefault();
  }

  function close() {
    if (dirty && !confirm('Discard unsaved changes?')) return;
    onclose();
  }

  onMount(() => {
    load().then(() => {
      meta = metaKey();
      // Let the workspace render and settle before taking the clean snapshot.
      setTimeout(() => {
        ws?.setErrors(errors);
        current = ws?.save() ?? current;
        onchange(current);
        baseline = JSON.stringify(current);
        refreshDirty();
      }, 50);
    });
    events.send({ type: 'watch', program_id: id }, `watch:${id}`);
    const off = events.on((ev) => {
      if (!('program_id' in ev) || ev.program_id !== id) return;
      switch (ev.type) {
        case 'status':
          if (program) program = { ...program, status: ev.status };
          break;
        case 'run_started':
          ws?.clearMarkers();
          break;
        case 'step':
          ws?.setMarker('active', ev.block_id);
          break;
        case 'paused':
          paused = { run_id: ev.run_id, block_id: ev.block_id, variables: ev.variables };
          ws?.setMarker('paused', ev.block_id);
          ws?.reveal(ev.block_id);
          panelOpen = true;
          break;
        case 'resumed':
          if (paused?.run_id === ev.run_id) paused = null;
          ws?.setMarker('paused', null);
          break;
        case 'run_finished':
          if (paused?.run_id === ev.run_id) paused = null;
          ws?.setMarker('active', null);
          ws?.setMarker('paused', null);
          if (ev.block_id) ws?.setMarker('failed', ev.block_id);
          break;
      }
    });
    window.addEventListener('keydown', onkey);
    window.addEventListener('beforeunload', guardLeave);
    return () => {
      off();
      events.send({ type: 'unwatch', program_id: id });
      events.unstick(`watch:${id}`);
      events.unstick(`bp:${id}`);
      window.removeEventListener('keydown', onkey);
      window.removeEventListener('beforeunload', guardLeave);
    };
  });
</script>

<div class="editor">
  <div class="bar">
    <button class="icon" onclick={close} aria-label="Back" title="Back to programs">←</button>
    <input class="name" bind:value={name} oninput={refreshDirty} aria-label="Program name" />
    <select bind:value={mode} onchange={refreshDirty} title="What happens when a trigger fires while the program is already running">
      <option value="single">single</option>
      <option value="restart">restart</option>
      <option value="queued">queued</option>
      <option value="parallel">parallel</option>
    </select>
    <label class="switch" title="Enabled">
      <input type="checkbox" bind:checked={enabled} onchange={refreshDirty} />
      <span></span>
    </label>
    <StatusChip status={program?.status} />
    <span class="spacer"></span>
    {#if message}<span class="message">{message}</span>{/if}
    <span class="count" title="Blocks in this program">{blockCount} blocks</span>
    <button class="btn" onclick={openHistory} title="Earlier saved versions of this program">History</button>
    <button class="btn" onclick={run} title="Run the program's first entry block now">Run</button>
    <button class="btn" onclick={() => api.cancel(id)} title="Cancel active runs">Stop</button>
    <button class="btn primary" onclick={save} disabled={saving}>{dirty ? 'Save*' : 'Save'}</button>
    <button class="icon" onclick={() => (panelOpen = !panelOpen)} title="Toggle side panel" aria-label="Toggle side panel">☰</button>
    <button class="icon danger" onclick={remove} title="Delete program" aria-label="Delete program">🗑</button>
  </div>
  <div class="body">
    <div class="canvas">
      {#if program && registry.loaded}
        <BlocklyWorkspace
          bind:this={ws}
          initial={program.workspace}
          extraCategories={registry.categories}
          extraStyles={registry.styles}
          {onchange}
          onbreakpoints={setBreakpoints}
          onrunstack={runStack}
        />
      {/if}
    </div>
    {#if panelOpen}
      <SidePanel
        programId={id}
        {errors}
        {paused}
        onresume={resume}
        onreveal={(b) => ws?.reveal(b)}
        describe={(b) => ws?.describe(b) ?? null}
        ontrace={(steps, errBlock) => {
          ws?.clearMarkers();
          if (steps.length) ws?.setMarker('active', steps[steps.length - 1]);
          if (errBlock) ws?.setMarker('failed', errBlock);
        }}
      />
    {/if}
  </div>
</div>

{#if history}
  <div class="backdrop" role="presentation" onclick={() => (history = null)}></div>
  <div class="history" role="dialog" aria-label="Program history">
    <h3>History</h3>
    <p class="hint">Every save that changed the blocks is kept (the last 30). Restoring loads it into the editor; save to keep it.</p>
    {#if history.length}
      <ul>
        {#each history as v, i (v.id)}
          <li>
            <span>
              {new Date(v.saved_at).toLocaleString()}
              {#if i === 0}<span class="hint"> · current</span>{/if}
              {#if v.name !== name}<span class="hint"> · named “{v.name}”</span>{/if}
            </span>
            <button class="btn" disabled={i === 0} onclick={() => restore(v)}>Restore</button>
          </li>
        {/each}
      </ul>
    {:else}
      <p class="hint">No saved versions yet.</p>
    {/if}
    <div class="actions"><button class="btn" onclick={() => (history = null)}>Close</button></div>
  </div>
{/if}

<style>
  .editor {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    border-bottom: 1px solid var(--pb-divider);
    background: var(--pb-card);
    flex-wrap: wrap;
  }
  .name {
    font-size: 16px;
    min-width: 160px;
    flex: 0 1 280px;
  }
  .spacer {
    flex: 1;
  }
  .message {
    font-size: 13px;
    color: var(--pb-text-2);
  }
  .count {
    font-size: 13px;
    color: var(--pb-text-2);
    white-space: nowrap;
  }
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 30%);
    z-index: 80;
  }
  .history {
    position: fixed;
    z-index: 81;
    left: 50%;
    top: 12%;
    transform: translateX(-50%);
    width: min(480px, calc(100% - 32px));
    max-height: 70vh;
    overflow: auto;
    padding: 16px 20px;
    background: var(--pb-card);
    border-radius: 12px;
    box-shadow: 0 8px 28px rgb(0 0 0 / 25%);
  }
  .history h3 {
    margin: 0 0 4px;
    font-weight: 500;
  }
  .history ul {
    list-style: none;
    padding: 0;
    margin: 12px 0;
  }
  .history li {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 6px 0;
    border-bottom: 1px solid var(--pb-divider);
  }
  .hint {
    color: var(--pb-text-2);
    font-size: 13px;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
  }
  .body {
    flex: 1;
    display: flex;
    min-height: 0;
  }
  .canvas {
    position: relative;
    flex: 1;
    min-width: 0;
  }
</style>
