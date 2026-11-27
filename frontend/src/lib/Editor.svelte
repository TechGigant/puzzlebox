<script lang="ts">
  import { onMount } from 'svelte';
  import BlocklyWorkspace from './BlocklyWorkspace.svelte';
  import StatusChip from './StatusChip.svelte';
  import SidePanel from './SidePanel.svelte';
  import { api, events, MODE_LABELS, type ActiveRun, type CompileError, type Program, type ProgramVersion, type RunMode } from './api';
  import { formatDateTime } from './locale';
  import WorkspacePreview from './WorkspacePreview.svelte';
  import { registry } from './registry.svelte';
  import { blockIds } from './runs';
  import { setRunAsTriggerHandler } from './editor/run_as_trigger';
  import { setLeaveGuard } from './leave';

  /** `id` is "new" for a program that is created on its first save. */
  let { id, onclose }: { id: string; onclose: () => void } = $props();

  // svelte-ignore state_referenced_locally
  let pid = $state<string | null>(id === 'new' ? null : id);
  let program = $state<Program | null>(null);
  let name = $state('');
  let mode = $state<RunMode>('single');
  let enabled = $state(true);
  let dirty = $state(false);
  let saving = $state(false);
  let message = $state<string | null>(null);
  let errors = $state<CompileError[]>([]);
  let hints = $state<CompileError[]>([]);
  let paused = $state<{ run_id: string; block_id: string; variables: Record<string, unknown> } | null>(null);
  // Phones have no room for the panel next to the blocks; there it opens over them.
  const narrow = () => window.matchMedia?.('(max-width: 700px)').matches ?? false;
  let panelOpen = $state(!narrow());
  let ws: BlocklyWorkspace | undefined = $state();
  let current: object = {};
  let baseline = '';
  let meta = '';
  // The enabled switch takes effect at once, so it isn't an unsaved change.
  const metaKey = () => JSON.stringify([name, mode]);
  let compileTimer: ReturnType<typeof setTimeout> | undefined;
  let blockCount = $state(0);
  let history = $state<ProgramVersion[] | null>(null);
  /** The version shown read-only in the history dialog. */
  let preview = $state<ProgramVersion | null>(null);
  let runMenu = $state<{ block_id: string; summary: string }[] | null>(null);
  const running = $derived((program?.status?.active_runs ?? 0) > 0);

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
    if (!pid) return flash('Nothing saved yet.');
    try {
      history = await api.versions(pid);
    } catch (e) {
      flash(String(e));
    }
  }

  async function showVersion(v: ProgramVersion) {
    if (!pid) return;
    try {
      preview = await api.version(pid, v.id);
    } catch (e) {
      flash(String(e));
    }
  }

  function closeHistory() {
    history = null;
    preview = null;
  }

  async function restore(v: ProgramVersion) {
    if (!pid) return;
    const full = v.workspace ? v : await api.version(pid, v.id);
    if (!full.workspace) return;
    if (dirty && !confirm('Replace your unsaved changes with this version?')) return;
    ws?.load(full.workspace);
    name = full.name;
    // Versions saved before run modes were kept have none.
    if (full.mode) mode = full.mode;
    closeHistory();
    // Loading doesn't count as an edit; mark the restored blocks as unsaved changes.
    setTimeout(() => ws && onchange(ws.save()), 10);
    flash(`Restored the version from ${formatDateTime(v.saved_at)}. Save to keep it.`);
  }

  async function load() {
    const p: Program = pid
      ? await api.program(pid)
      : {
          id: '',
          name: 'New program',
          description: '',
          enabled: true,
          mode: 'single',
          max_runs: 10,
          workspace: {},
          folder: '',
          created_at: 0,
          updated_at: 0,
          status: null,
        };
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
    hints = ws?.hints() ?? [];
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

  /** Each block's text as the editor shows it, kept so runs read the way the blocks did. */
  function blockLabels(state: object): Record<string, string> {
    const out: Record<string, string> = {};
    for (const b of blockIds((state as { blocks?: unknown }).blocks)) {
      const text = ws?.describe(b);
      if (text) out[b] = text;
    }
    return out;
  }

  async function save(): Promise<boolean> {
    if (!program) return false;
    saving = true;
    try {
      const workspace = ws?.save() ?? current;
      const body = { name, mode, enabled, workspace, labels: blockLabels(workspace) };
      let saved: Program;
      if (pid) {
        saved = await api.update(pid, body);
      } else {
        saved = await api.create(body);
        pid = saved.id;
        watch(saved.id);
        // Without a hashchange, so the editor (and its undo history) stays as it is.
        window.history.replaceState(null, '', `#/p/${saved.id}`);
      }
      program = saved;
      baseline = JSON.stringify(ws?.save() ?? current);
      meta = metaKey();
      errors = saved.errors ?? [];
      ws?.setErrors(errors);
      dirty = false;
      flash(
        errors.length
          ? `Saved. ${errors.length === 1 ? '1 problem' : `${errors.length} problems`}: those stacks won't run until fixed, the rest will.`
          : 'Saved',
      );
      return true;
    } catch (e) {
      flash(`Save failed: ${e}`);
      return false;
    } finally {
      saving = false;
    }
  }

  /** Saves first if needed; the program's id, or null if it couldn't be saved. */
  async function saved(): Promise<string | null> {
    if ((dirty || !pid) && !(await save())) return null;
    return pid;
  }

  async function run() {
    runMenu = null;
    const p = await saved();
    if (!p) return;
    try {
      await api.run(p);
    } catch (e) {
      flash(String(e));
    }
  }

  async function openRunMenu() {
    if (runMenu) return (runMenu = null);
    const p = await saved();
    if (!p) return;
    runMenu = await api.triggers(p).catch(() => []);
  }

  async function runAsTrigger(block: string) {
    runMenu = null;
    const p = await saved();
    if (!p) return;
    try {
      await api.runAsTrigger(p, block);
      flash('Running');
    } catch (e) {
      flash(String(e));
    }
  }

  async function runStack(topBlockId: string) {
    const p = await saved();
    if (!p) return;
    try {
      await api.runStack(p, topBlockId);
      flash('Running');
    } catch (e) {
      flash(String(e));
    }
  }

  async function stop() {
    if (!pid) return;
    try {
      const r = await api.cancel(pid);
      flash(r.cancelled ? `Stopped ${r.cancelled === 1 ? 'the run' : `${r.cancelled} runs`}` : 'Nothing was running');
    } catch (e) {
      flash(String(e));
    }
  }

  async function toggleEnabled() {
    if (!pid) return;
    try {
      const updated = await api.setEnabled(pid, enabled);
      if (program) program = { ...program, enabled: updated.enabled, status: updated.status };
      flash(enabled ? 'Turned on' : 'Turned off: triggers are ignored until you turn it on');
    } catch (e) {
      enabled = !enabled;
      flash(String(e));
    }
  }

  async function remove() {
    if (!pid) return onclose();
    if (!confirm(`Move “${name}” to the recycle bin? You can restore it from the program list for 30 days.`)) return;
    await api.remove(pid);
    dirty = false;
    onclose();
  }

  /** Marks the block a run is at, e.g. when the editor opens during a long wait. */
  function showActive(runs: ActiveRun[]) {
    const r = runs[runs.length - 1];
    const at = r?.waiting?.block_id ?? r?.block_id;
    if (at) ws?.setMarker('active', at);
  }

  function watch(p: string) {
    events.send({ type: 'watch', program_id: p }, `watch:${p}`);
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
    if (pid) events.send({ type: 'breakpoints', program_id: pid, blocks }, `bp:${pid}`);
  }

  function resume(step: boolean) {
    if (!paused) return;
    events.send({ type: step ? 'step' : 'resume', run_id: paused.run_id });
  }

  function guardLeave(e: BeforeUnloadEvent) {
    if (dirty) e.preventDefault();
  }

  // The app asks this before any navigation away (header links, Back, the ← button).
  const mayDiscard = () => !dirty || confirm('Discard unsaved changes?');

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
        if (pid) api.active(pid).then(showActive, () => {});
      }, 50);
    });
    if (pid) watch(pid);
    const offRunAs = setRunAsTriggerHandler((block) => runAsTrigger(block));
    const off = events.on((ev) => {
      if (!('program_id' in ev) || !pid || ev.program_id !== pid) return;
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
    const stopGuard = setLeaveGuard(mayDiscard);
    return () => {
      stopGuard();
      off();
      offRunAs();
      if (pid) {
        events.send({ type: 'unwatch', program_id: pid });
        events.unstick(`watch:${pid}`);
        events.unstick(`bp:${pid}`);
      }
      window.removeEventListener('keydown', onkey);
      window.removeEventListener('beforeunload', guardLeave);
    };
  });
</script>

<div class="editor">
  <div class="bar">
    <button class="icon" onclick={onclose} aria-label="Back" title="Back to programs">←</button>
    <input class="name" bind:value={name} oninput={refreshDirty} aria-label="Program name" />
    <select bind:value={mode} onchange={refreshDirty} title="What happens when a trigger fires while the program is already running">
      {#each Object.entries(MODE_LABELS) as [value, label] (value)}<option {value}>{label}</option>{/each}
    </select>
    <label class="switch" title={pid ? (enabled ? 'On: click to turn off' : 'Off: click to turn on') : 'Starts when you save'}>
      <input type="checkbox" bind:checked={enabled} onchange={toggleEnabled} />
      <span></span>
    </label>
    <StatusChip status={program?.status} />
    <span class="spacer"></span>
    {#if message}<span class="message">{message}</span>{/if}
    <span class="count" title="Blocks in this program">{blockCount === 1 ? '1 block' : `${blockCount} blocks`}</span>
    <button class="btn" onclick={openHistory} disabled={!pid} title="Earlier saved versions of this program">History</button>
    <span class="split">
      <button class="btn" onclick={run} title="Run the program's first entry block now (trigger: run manually)">Run</button>
      <button class="btn more" onclick={openRunMenu} title="Run as if a trigger fired" aria-label="More ways to run" aria-expanded={!!runMenu}>▾</button>
      {#if runMenu}
        <div class="menu" role="menu">
          <button role="menuitem" onclick={run}>Run now <span class="hint">(trigger: run manually)</span></button>
          {#each runMenu as t (t.block_id)}
            <button role="menuitem" onclick={() => runAsTrigger(t.block_id)}>Run as if ⚡ {t.summary}</button>
          {:else}
            <p class="hint">No triggers to run as. Is the program turned on and free of problems?</p>
          {/each}
        </div>
      {/if}
    </span>
    <button class="btn" onclick={stop} disabled={!running} title={running ? 'Stop the runs in progress' : 'Nothing is running'}>Stop</button>
    <button class="btn primary" onclick={save} disabled={saving}>{dirty || !pid ? 'Save*' : 'Save'}</button>
    <button class="icon" onclick={() => (panelOpen = !panelOpen)} title="Toggle side panel" aria-label="Toggle side panel">☰</button>
    <button class="icon danger" onclick={remove} title="Move to the recycle bin" aria-label="Delete program">🗑</button>
  </div>
  {#if runMenu}<div class="menu-backdrop" role="presentation" onclick={() => (runMenu = null)}></div>{/if}
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
        programId={pid}
        onactive={showActive}
        errors={[...(ws?.friendlyErrors(errors) ?? errors), ...hints]}
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

{#if history && preview}
  <div class="backdrop" role="presentation" onclick={closeHistory}></div>
  <div class="history wide" role="dialog" aria-label="Earlier version">
    <h3>Version from {formatDateTime(preview.saved_at)}</h3>
    <p class="hint">
      {[`Named “${preview.name}”`, preview.mode && (MODE_LABELS[preview.mode] ?? preview.mode), `${preview.blocks ?? 0} ${preview.blocks === 1 ? 'block' : 'blocks'}`]
        .filter(Boolean)
        .join(' · ')}. Only to look at: restoring loads it into the editor.
    </p>
    {#if preview.workspace}
      <WorkspacePreview workspace={preview.workspace} extraStyles={registry.styles} />
    {/if}
    <div class="actions">
      <button class="btn" onclick={() => (preview = null)}>Back</button>
      <button class="btn primary" disabled={preview.id === history[0]?.id} onclick={() => restore(preview!)}>Restore this version</button>
    </div>
  </div>
{:else if history}
  <div class="backdrop" role="presentation" onclick={closeHistory}></div>
  <div class="history" role="dialog" aria-label="Program history">
    <h3>History</h3>
    <p class="hint">
      Every save that changed the blocks, name or run mode is kept (the last 30). Show one to look at it; restoring loads
      its blocks, name and run mode into the editor, and you save to keep it.
    </p>
    {#if history.length}
      <ul>
        {#each history as v, i (v.id)}
          <li>
            <span>
              {formatDateTime(v.saved_at)}
              {#if i === 0}<span class="hint"> · current</span>{/if}
              {#if v.name !== name}<span class="hint"> · named “{v.name}”</span>{/if}
              <span class="hint changes">
                {`${v.blocks ?? 0} ${v.blocks === 1 ? 'block' : 'blocks'}${v.changes ? ` · ${v.changes}` : ''}`}
              </span>
            </span>
            <span class="buttons">
              <button class="btn" onclick={() => showVersion(v)}>Show</button>
              <button class="btn" disabled={i === 0} onclick={() => restore(v)}>Restore</button>
            </span>
          </li>
        {/each}
      </ul>
    {:else}
      <p class="hint">No saved versions yet.</p>
    {/if}
    <div class="actions"><button class="btn" onclick={closeHistory}>Close</button></div>
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
  .split {
    position: relative;
    display: inline-flex;
  }
  .split > .btn:first-child {
    border-top-right-radius: 0;
    border-bottom-right-radius: 0;
  }
  .split > .more {
    border-top-left-radius: 0;
    border-bottom-left-radius: 0;
    border-left: 1px solid var(--pb-divider);
    padding-left: 8px;
    padding-right: 8px;
  }
  .menu {
    position: absolute;
    z-index: 61;
    top: calc(100% + 4px);
    right: 0;
    min-width: 260px;
    max-width: min(420px, calc(100vw - 32px));
    padding: 4px;
    background: var(--pb-card);
    border: 1px solid var(--pb-divider);
    border-radius: 8px;
    box-shadow: 0 4px 16px rgb(0 0 0 / 20%);
  }
  .menu button {
    display: block;
    width: 100%;
    text-align: left;
    padding: 8px 10px;
    background: none;
    border: 0;
    border-radius: 6px;
    color: inherit;
    font: inherit;
    cursor: pointer;
  }
  .menu button:hover {
    background: color-mix(in srgb, var(--pb-text) 6%, transparent);
  }
  .menu p {
    margin: 8px 10px;
  }
  .menu-backdrop {
    position: fixed;
    inset: 0;
    z-index: 60;
  }
  .changes {
    display: block;
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
  .history.wide {
    width: min(960px, calc(100% - 32px));
    top: 6%;
    max-height: 88vh;
  }
  .history .buttons {
    display: flex;
    gap: 6px;
    flex-shrink: 0;
  }
  .history .actions {
    gap: 8px;
    margin-top: 12px;
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
  @media (max-width: 700px) {
    .bar {
      gap: 6px;
      padding: 6px 8px;
    }
    .name {
      flex: 1 1 160px;
      min-width: 0;
    }
    .bar select {
      flex: 1 1 180px;
      min-width: 0;
    }
    .bar .btn {
      padding: 6px 9px;
    }
    .bar .icon {
      padding-inline: 4px;
    }
    .count {
      display: none;
    }
    .body {
      position: relative;
    }
    .body > :global(.panel) {
      position: absolute;
      z-index: 70;
      left: 0;
      right: 0;
      bottom: 0;
      width: auto;
      height: min(60%, 420px);
      border-left: 0;
      border-top: 1px solid var(--pb-divider);
      border-radius: 12px 12px 0 0;
      box-shadow: 0 -4px 16px rgb(0 0 0 / 20%);
    }
  }
</style>
