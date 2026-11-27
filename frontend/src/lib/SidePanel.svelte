<script lang="ts">
  import { api, events, type ActiveRun, type CompileError, type LogEntry, type RunRecord } from './api';
  import { activity, triggerSummary } from './runs';

  let {
    programId,
    errors,
    paused,
    onresume,
    onreveal,
    ontrace,
    onactive,
    describe = () => null,
  }: {
    /** null for a program that hasn't been saved yet. */
    programId: string | null;
    errors: CompileError[];
    paused: { run_id: string; block_id: string; variables: Record<string, unknown> } | null;
    onresume: (step: boolean) => void;
    onreveal: (blockId: string) => void;
    ontrace: (steps: string[], errorBlock: string | null) => void;
    /** Runs in progress changed. */
    onactive?: (runs: ActiveRun[]) => void;
    describe?: (blockId: string) => string | null;
  } = $props();

  type Tab = 'runs' | 'log' | 'debug';
  let tab = $state<Tab>('runs');
  let runs = $state<RunRecord[]>([]);
  let active = $state<ActiveRun[]>([]);
  let selected = $state<RunRecord | null>(null);
  let showRaw = $state(false);
  let logs = $state<LogEntry[]>([]);
  let pauseOnStart = $state(false);
  let now = $state(Date.now());

  $effect(() => {
    if (paused) tab = 'debug';
  });

  $effect(() => {
    if (!active.length) return;
    const t = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(t);
  });

  async function refreshRuns() {
    if (!programId) return;
    runs = await api.runs(programId).catch(() => []);
  }

  async function refreshActive() {
    if (!programId) return;
    active = await api.active(programId).catch(() => []);
    now = Date.now();
    onactive?.(active);
  }

  async function select(r: RunRecord) {
    if (selected?.id === r.id) {
      selected = null;
      ontrace([], null);
      return;
    }
    selected = await api.runDetail(r.id);
    showRaw = false;
    const steps = selected.trace?.steps.map((s) => s.block) ?? [];
    ontrace(steps, selected.trace?.error_block ?? null);
  }

  /** A step as its block read when the run ran; today's text if it wasn't kept. */
  function stepLabel(r: RunRecord, block: string): string {
    return r.trace?.labels?.[block] ?? describe(block) ?? block;
  }

  const STATUS: Record<string, string> = { ok: 'done', error: 'failed', cancelled: 'stopped', running: 'running' };

  function togglePauseOnStart() {
    if (programId) events.send({ type: 'pause_on_start', program_id: programId, enabled: pauseOnStart }, `pos:${programId}`);
  }

  function fmtTime(ms: number) {
    return new Date(ms).toLocaleTimeString();
  }

  function fmtValue(v: unknown) {
    const s = JSON.stringify(v);
    return s && s.length > 120 ? `${s.slice(0, 120)}…` : s;
  }

  $effect(() => {
    if (!programId) return;
    const id = programId;
    refreshRuns();
    refreshActive();
    api
      .log(id)
      .then((l) => (logs = l))
      .catch(() => {});
    const off = events.on((ev) => {
      if (!('program_id' in ev) || ev.program_id !== id) return;
      if (ev.type === 'run_finished') {
        refreshRuns();
        refreshActive();
      }
      if (ev.type === 'run_started' || ev.type === 'waiting') refreshActive();
      if (ev.type === 'step') active = active.map((r) => (r.run_id === ev.run_id ? { ...r, block_id: ev.block_id } : r));
      if (ev.type === 'log') logs = [...logs.slice(-199), { ts: ev.ts, level: ev.level, message: ev.message, run_id: ev.run_id }];
    });
    return () => {
      off();
      events.unstick(`pos:${id}`);
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
        <button class="problem" onclick={() => e.block_id && onreveal(e.block_id)}>⚠ {e.message}</button>
      {/each}
    </section>
  {/if}

  <div class="content">
    {#if tab === 'runs'}
      {#if !programId}<p class="muted">Save the program to run it.</p>{/if}
      {#if active.length}
        <h3>Running now</h3>
        {#each active as a (a.run_id)}
          {@const at = a.waiting?.block_id ?? a.block_id}
          <div class="active">
            <div><span class="st running">running</span> <span class="muted">since {fmtTime(a.started_at)}</span></div>
            <div class="why">{a.summary}</div>
            <div class="now">
              {activity(a, now, describe)}
              {#if at}<button class="link" onclick={() => onreveal(at)}>show block</button>{/if}
            </div>
          </div>
        {/each}
        <h3>Finished</h3>
      {/if}
      {#if programId && runs.length === 0}<p class="muted">{active.length ? 'None yet.' : 'No runs yet.'}</p>{/if}
      {#each runs as r (r.id)}
        <button class="run" class:sel={selected?.id === r.id} onclick={() => select(r)}>
          <span class="st {r.status}">{STATUS[r.status] ?? r.status}</span>
          <span class="when">{fmtTime(r.started_at)}<span class="why">{triggerSummary(r)}</span></span>
          {#if r.finished_at}<span class="muted">{r.finished_at - r.started_at} ms</span>{/if}
        </button>
        {#if selected?.id === r.id && selected.trace}
          {@const vars = Object.entries(selected.trace.variables ?? {})}
          <div class="detail">
            {#if selected.error}<p class="err">{selected.error}</p>{/if}
            <p class="started">
              Started: {triggerSummary(selected)}
              <button class="link" onclick={() => (showRaw = !showRaw)}>{showRaw ? 'hide details' : 'details'}</button>
            </p>
            {#if showRaw}<pre>{JSON.stringify(selected.trigger, null, 2)}</pre>{/if}
            {#if vars.length}
              <h4>Variables at end</h4>
              <table>
                <tbody>
                  {#each vars as [k, v] (k)}<tr><td>{k}</td><td><code>{fmtValue(v)}</code></td></tr>{/each}
                </tbody>
              </table>
            {/if}
            <h4>Steps ({selected.trace.steps.length}{selected.trace.truncated ? '+' : ''})</h4>
            <ol class="steps">
              {#each selected.trace.steps.slice(0, 300) as s, i (i)}
                <li>
                  <button onclick={() => onreveal(s.block)} class:err={s.error}>
                    <span class="t">{s.t} ms</span>
                    <span>
                      {s.error ? `${stepLabel(selected, s.block)}: ${s.error}` : stepLabel(selected, s.block)}
                      {#if s.note}<span class="note">→ {s.note}</span>{/if}
                      {#if !describe(s.block)}<span class="muted"> (no longer in the program)</span>{/if}
                    </span>
                  </button>
                </li>
              {/each}
            </ol>
          </div>
        {/if}
      {/each}
    {:else if tab === 'log'}
      {#if logs.length === 0}
        <p class="muted">Nothing logged yet. Messages from “log” blocks, warnings (such as conditions that failed and counted as false, or errors a block carried on after) and failed runs appear here.</p>
      {/if}
      {#each logs as l, i (i)}
        <div class="log {l.level}"><span class="muted">{fmtTime(l.ts)}</span> {l.message}</div>
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
  .when {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .why {
    color: var(--pb-text-2);
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .active {
    padding: 6px 4px 8px;
    border-radius: 6px;
    background: color-mix(in srgb, var(--pb-primary) 8%, transparent);
    margin-bottom: 6px;
    display: grid;
    gap: 2px;
  }
  .now {
    font-weight: 500;
  }
  .st.running {
    color: var(--pb-primary);
  }
  .started {
    margin: 4px 0;
  }
  .steps .t {
    color: var(--pb-text-2);
    min-width: 52px;
    flex-shrink: 0;
  }
  .note {
    color: var(--pb-primary);
    margin-left: 4px;
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
