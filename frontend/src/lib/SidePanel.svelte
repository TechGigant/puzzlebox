<script lang="ts">
  import { onMount } from 'svelte';
  import { api, events, type CompileError, type RunRecord } from './api';
  import { plainError } from './ha_errors';

  let {
    programId,
    errors,
    paused,
    onresume,
    onreveal,
    ontrace,
    describe = () => null,
  }: {
    programId: string;
    errors: CompileError[];
    paused: { run_id: string; block_id: string; variables: Record<string, unknown> } | null;
    onresume: (step: boolean) => void;
    onreveal: (blockId: string) => void;
    ontrace: (steps: string[], errorBlock: string | null) => void;
    describe?: (blockId: string) => string | null;
  } = $props();

  type Tab = 'runs' | 'log' | 'debug';
  let tab = $state<Tab>('runs');
  let runs = $state<RunRecord[]>([]);
  let selected = $state<RunRecord | null>(null);
  let logs = $state<{ ts: number; level: string; message: string }[]>([]);
  let pauseOnStart = $state(false);

  $effect(() => {
    if (paused) tab = 'debug';
  });

  async function refreshRuns() {
    runs = await api.runs(programId).catch(() => []);
  }

  async function select(r: RunRecord) {
    selected = await api.runDetail(r.id);
    const steps = selected.trace?.steps.map((s) => s.block) ?? [];
    ontrace(steps, selected.trace?.error_block ?? null);
  }

  function togglePauseOnStart() {
    events.send({ type: 'pause_on_start', program_id: programId, enabled: pauseOnStart }, `pos:${programId}`);
  }

  function fmtTime(ms: number) {
    return new Date(ms).toLocaleTimeString();
  }

  function fmtValue(v: unknown) {
    const s = JSON.stringify(v);
    return s && s.length > 120 ? `${s.slice(0, 120)}…` : s;
  }

  onMount(() => {
    refreshRuns();
    const off = events.on((ev) => {
      if (!('program_id' in ev) || ev.program_id !== programId) return;
      if (ev.type === 'run_finished') refreshRuns();
      if (ev.type === 'log') logs = [...logs.slice(-499), { ts: ev.ts, level: ev.level, message: ev.message }];
    });
    return () => {
      off();
      events.unstick(`pos:${programId}`);
    };
  });
</script>

<aside class="panel">
  <nav class="tabs">
    <button class:active={tab === 'runs'} onclick={() => (tab = 'runs')}>Runs</button>
    <button class:active={tab === 'log'} onclick={() => (tab = 'log')}>Log</button>
    <button class:active={tab === 'debug'} onclick={() => (tab = 'debug')}>Debug{#if paused}<span class="badge">●</span>{/if}</button>
  </nav>

  {#if errors.length}
    <section class="problems">
      <h3>Problems</h3>
      {#each errors as e, i (i)}
        <button class="problem" class:warn={e.warning} onclick={() => e.block_id && onreveal(e.block_id)}>⚠ {e.message}</button>
      {/each}
    </section>
  {/if}

  <div class="content">
    {#if tab === 'runs'}
      {#if runs.length === 0}<p class="muted">No runs yet.</p>{/if}
      {#each runs as r (r.id)}
        <button class="run" class:sel={selected?.id === r.id} onclick={() => select(r)}>
          <span class="st {r.status}">{r.status}</span>
          <span>{fmtTime(r.started_at)}</span>
          {#if r.finished_at}<span class="muted">{r.finished_at - r.started_at} ms</span>{/if}
        </button>
        {#if selected?.id === r.id && selected.trace}
          <div class="detail">
            {#if selected.error}
              {@const plain = plainError(selected.error)}
              {#if plain}
                <p class="err">{plain}</p>
                <details class="muted"><summary>details</summary>{selected.error}</details>
              {:else}
                <p class="err">{selected.error}</p>
              {/if}
            {/if}
            <h4>Trigger</h4>
            <pre>{JSON.stringify(selected.trigger, null, 2)}</pre>
            <h4>Variables at end</h4>
            <pre>{JSON.stringify(selected.trace.variables, null, 2)}</pre>
            <h4>Steps ({selected.trace.steps.length}{selected.trace.truncated ? '+' : ''})</h4>
            <ol class="steps">
              {#each selected.trace.steps.slice(0, 300) as s, i (i)}
                <li><button onclick={() => onreveal(s.block)} class:err={s.error} title={s.error ?? undefined}>{s.t} ms · {(s.error && plainError(s.error)) ?? s.error ?? describe(s.block) ?? s.block}</button></li>
              {/each}
            </ol>
          </div>
        {/if}
      {/each}
    {:else if tab === 'log'}
      {#if logs.length === 0}<p class="muted">Log messages from "log" blocks appear here while the editor is open.</p>{/if}
      {#each logs as l, i (i)}
        <div class="log {l.level}" title={plainError(l.message) ? l.message : undefined}><span class="muted">{fmtTime(l.ts)}</span> {plainError(l.message) ?? l.message}</div>
      {/each}
    {:else}
      <p class="muted">Right-click a block → <b>Add breakpoint</b>. Runs pause there while this editor is open.</p>
      <label class="check"><input type="checkbox" bind:checked={pauseOnStart} onchange={togglePauseOnStart} /> Pause every run at its first block</label>
      {#if paused}
        <div class="paused">
          <p><b>Paused</b> <button class="link" onclick={() => onreveal(paused!.block_id)}>show block</button></p>
          <div class="row">
            <button class="btn primary" onclick={() => onresume(false)}>Continue</button>
            <button class="btn" onclick={() => onresume(true)}>Step</button>
          </div>
          <h4>Variables</h4>
          <table>
            <tbody>
              {#each Object.entries(paused.variables) as [k, v] (k)}
                <tr><td>{k}</td><td><code>{fmtValue(v)}</code></td></tr>
              {:else}
                <tr><td class="muted">none</td></tr>
              {/each}
            </tbody>
          </table>
        </div>
      {:else}
        <p class="muted">Not paused.</p>
      {/if}
    {/if}
  </div>
</aside>

<style>
  .panel {
    width: 320px;
    flex-shrink: 0;
    border-left: 1px solid var(--pb-divider);
    background: var(--pb-card);
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .tabs {
    display: flex;
    border-bottom: 1px solid var(--pb-divider);
  }
  .tabs button {
    flex: 1;
    padding: 12px;
    background: none;
    border: 0;
    border-bottom: 2px solid transparent;
    color: var(--pb-text-2);
    cursor: pointer;
    font: inherit;
    font-weight: 500;
  }
  .tabs button.active {
    color: var(--pb-primary);
    border-bottom-color: var(--pb-primary);
  }
  .badge {
    color: #e53935;
    margin-left: 4px;
  }
  .content {
    overflow: auto;
    padding: 8px 12px;
    flex: 1;
    font-size: 13px;
  }
  .problems {
    padding: 8px 12px;
    border-bottom: 1px solid var(--pb-divider);
    max-height: 30%;
    overflow: auto;
  }
  h3,
  h4 {
    margin: 8px 0 4px;
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--pb-text-2);
  }
  .problem,
  .run,
  .steps button,
  .link {
    display: flex;
    gap: 8px;
    width: 100%;
    text-align: left;
    background: none;
    border: 0;
    color: inherit;
    font: inherit;
    padding: 6px 4px;
    border-radius: 6px;
    cursor: pointer;
  }
  .link {
    display: inline;
    width: auto;
    color: var(--pb-primary);
  }
  .problem:hover,
  .run:hover,
  .steps button:hover {
    background: color-mix(in srgb, var(--pb-text) 6%, transparent);
  }
  .run.sel {
    background: color-mix(in srgb, var(--pb-primary) 12%, transparent);
  }
  .st {
    font-weight: 500;
    min-width: 64px;
  }
  .st.ok {
    color: #43a047;
  }
  .st.error,
  .err {
    color: #e53935;
  }
  .problem.warn {
    opacity: 0.8;
  }
  .st.cancelled {
    color: var(--pb-text-2);
  }
  .muted {
    color: var(--pb-text-2);
  }
  pre {
    background: color-mix(in srgb, var(--pb-text) 5%, transparent);
    padding: 8px;
    border-radius: 6px;
    overflow: auto;
    max-height: 200px;
    margin: 0;
    font-size: 12px;
  }
  .steps {
    padding-left: 0;
    list-style: none;
    margin: 0;
  }
  .detail {
    padding: 0 4px 12px;
  }
  .log {
    padding: 2px 0;
    font-family: ui-monospace, monospace;
    font-size: 12px;
    word-break: break-word;
  }
  .log.warning {
    color: #fb8c00;
  }
  .log.error {
    color: #e53935;
  }
  .check {
    display: flex;
    gap: 8px;
    align-items: center;
    margin: 8px 0;
  }
  .row {
    display: flex;
    gap: 8px;
  }
  table {
    width: 100%;
    border-collapse: collapse;
  }
  td {
    padding: 4px;
    border-bottom: 1px solid var(--pb-divider);
    vertical-align: top;
    word-break: break-all;
  }
  td:first-child {
    font-weight: 500;
    width: 35%;
  }
</style>
