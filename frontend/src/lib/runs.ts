import type { ActiveRun, RunRecord } from './api';

/** "22 s", "4 min 10 s", "1 h 5 min". */
export function fmtDuration(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 60) return `${s} s`;
  if (s < 3600) return s % 60 ? `${Math.floor(s / 60)} min ${s % 60} s` : `${s / 60} min`;
  const m = Math.floor((s % 3600) / 60);
  return m ? `${Math.floor(s / 3600)} h ${m} min` : `${Math.floor(s / 3600)} h`;
}

/** What a run is doing now: "waiting 30 s, 22 s left". */
export function activity(run: ActiveRun, now: number, describe: (id: string) => string | null): string {
  const w = run.waiting;
  if (w) return w.until ? `${w.text}, ${fmtDuration(w.until - now)} left` : w.text;
  const at = run.block_id ? describe(run.block_id) : null;
  return at ? `at “${at}”` : 'running';
}

/** What started a run, for runs stored before the app described them. */
export function triggerSummary(run: Pick<RunRecord, 'summary' | 'trigger'>): string {
  if (run.summary) return run.summary;
  const t = (run.trigger ?? {}) as Record<string, unknown>;
  const platform = String(t.platform ?? t.trigger ?? '');
  if (platform === 'manual') return 'run manually';
  if (platform === 'start') return 'the program started';
  if (platform === 'timer') return `timer “${t.timer}” went off`;
  const to = t.to_state as { state?: string; attributes?: { friendly_name?: string } } | undefined;
  if (platform === 'state' && to) {
    const who = to.attributes?.friendly_name ?? String(t.entity_id ?? '');
    return to.state === 'on' || to.state === 'off' ? `${who} turned ${to.state}` : `${who} changed to ${to.state}`;
  }
  return typeof t.description === 'string' && t.description ? t.description : platform || 'something';
}

/** Ids of every block (not shadows) in a saved Blockly workspace. */
export function blockIds(node: unknown, out: string[] = []): string[] {
  if (!node || typeof node !== 'object') return out;
  if (Array.isArray(node)) {
    for (const x of node) blockIds(x, out);
    return out;
  }
  const o = node as Record<string, unknown>;
  if (typeof o.type === 'string' && typeof o.id === 'string') out.push(o.id);
  for (const [k, v] of Object.entries(o)) if (k !== 'shadow') blockIds(v, out);
  return out;
}
