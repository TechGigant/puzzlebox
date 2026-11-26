//! Index of every block the toolbox offers, for "type to add a block".

import * as Blockly from 'blockly/core';

type State = Blockly.serialization.blocks.State;
type Item = Blockly.utils.toolbox.ToolboxItemInfo;

export interface Entry {
  /** What the block says, with its default field values ("turn on light.kitchen"). */
  label: string;
  /** Category path ("Home Assistant › Light"). */
  path: string;
  state: State;
  haystack: string;
}

function stateOf(item: Item): State | null {
  if ((item as { kind?: string }).kind !== 'block') return null;
  const { type, inputs, fields, extraState } = item as unknown as State;
  if (!type) return null;
  return { type, ...(inputs ? { inputs } : {}), ...(fields ? { fields } : {}), ...(extraState ? { extraState } : {}) };
}

function walk(items: Item[], path: string[], out: { state: State; path: string }[]) {
  for (const item of items) {
    const i = item as { kind?: string; name?: string; contents?: Item[] };
    if (i.kind === 'category' && i.contents) walk(i.contents, [...path, i.name ?? ''], out);
    const s = stateOf(item);
    if (s) out.push({ state: s, path: path.join(' › ') });
  }
}

let scratch: Blockly.Workspace | null = null;

/** Renders a block's text without showing it (headless workspace). */
function describe(state: State): string | null {
  scratch ??= new Blockly.Workspace();
  Blockly.Events.disable();
  try {
    const b = Blockly.serialization.blocks.append(state, scratch, { recordUndo: false });
    // Drop the "＋ option" menus of generated blocks; they're not what the block does.
    const text = b.toString(80).replace(/[＋+] option/g, '').replace(/\s+/g, ' ').trim();
    b.dispose(false);
    return text || state.type;
  } catch {
    return null;
  } finally {
    Blockly.Events.enable();
  }
}

const cache = new WeakMap<object, Entry[]>();

/** Entries from a toolbox definition (cached per definition object). */
export function toolboxEntries(toolbox: Blockly.utils.toolbox.ToolboxInfo): Entry[] {
  const hit = cache.get(toolbox);
  if (hit) return hit;
  const raw: { state: State; path: string }[] = [];
  walk((toolbox as { contents: Item[] }).contents, [], raw);
  const seen = new Set<string>();
  const entries: Entry[] = [];
  for (const { state, path } of raw) {
    const key = JSON.stringify(state);
    if (seen.has(key)) continue;
    seen.add(key);
    const label = describe(state);
    if (label) entries.push({ label, path, state, haystack: `${label} ${path} ${state.type}`.toLowerCase() });
  }
  cache.set(toolbox, entries);
  return entries;
}

/** Variables and functions of the workspace (these categories are built on the fly). */
export function workspaceEntries(ws: Blockly.WorkspaceSvg): Entry[] {
  const out: Entry[] = [];
  const add = (label: string, path: string, state: State) =>
    out.push({ label, path, state, haystack: `${label} ${path} ${state.type}`.toLowerCase() });
  for (const v of ws.getVariableMap().getAllVariables()) {
    const name = v.getName();
    const kind = v.getType() === 'Global' ? 'Stored (global)' : v.getType() === 'Program' ? 'Stored' : 'Variables';
    add(name, kind, { type: 'variables_get', fields: { VAR: { id: v.getId() } } });
    add(`set ${name} to`, kind, { type: 'variables_set', fields: { VAR: { id: v.getId() } } });
  }
  const [plain, returning] = Blockly.Procedures.allProcedures(ws);
  for (const [name, params] of plain) {
    add(`${name}${params.length ? ' with ' + params.join(', ') : ''}`, 'Functions', {
      type: 'procedures_callnoreturn',
      extraState: { name, params },
    });
  }
  for (const [name, params] of returning) {
    add(`${name}${params.length ? ' with ' + params.join(', ') : ''}`, 'Functions', {
      type: 'procedures_callreturn',
      extraState: { name, params },
    });
  }
  return out;
}

/** Every query word must appear; earlier and word-start matches in the label rank higher. */
export function search(entries: Entry[], query: string, limit = 40): Entry[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return [];
  const scored: [number, Entry][] = [];
  for (const e of entries) {
    let score = 0;
    let ok = true;
    const label = e.label.toLowerCase();
    for (const w of words) {
      if (!e.haystack.includes(w)) {
        ok = false;
        break;
      }
      const at = label.indexOf(w);
      if (at === 0) score += 6;
      else if (at > 0 && /\W/.test(label[at - 1])) score += 4;
      else if (at > 0) score += 2;
      else score += 1;
    }
    if (ok) scored.push([score - label.length / 100, e]);
  }
  return scored
    .sort((a, b) => b[0] - a[0])
    .slice(0, limit)
    .map(([, e]) => e);
}

/** Creates the block, attached after/into the selected block when it fits, else at `at`. */
export function insert(ws: Blockly.WorkspaceSvg, state: State, at: Blockly.utils.Coordinate) {
  Blockly.Events.setGroup(true);
  try {
    const block = Blockly.serialization.blocks.append(state, ws) as Blockly.BlockSvg;
    const selected = Blockly.getSelected();
    const target = selected instanceof Blockly.BlockSvg && !selected.isInFlyout && selected.workspace === ws ? selected : null;
    if (!(target && attach(target, block))) block.moveTo(at);
    block.select();
    ws.scrollBoundsIntoView(block.getBoundingRectangleWithoutChildren());
    return block;
  } finally {
    Blockly.Events.setGroup(false);
  }
}

function attach(target: Blockly.BlockSvg, block: Blockly.BlockSvg): boolean {
  const checker = target.workspace.connectionChecker;
  const tryConnect = (a: Blockly.Connection | null, b: Blockly.Connection | null) => {
    if (!a || !b || !checker.canConnect(a, b, false)) return false;
    a.connect(b);
    return true;
  };
  if (tryConnect(target.nextConnection, block.previousConnection)) return true;
  for (const input of target.inputList) {
    const c = input.connection;
    if (!c || c.targetBlock()) continue;
    if (tryConnect(c, block.outputConnection) || tryConnect(c, block.previousConnection)) return true;
  }
  return false;
}
