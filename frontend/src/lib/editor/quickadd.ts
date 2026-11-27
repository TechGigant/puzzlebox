//! Index of every block the toolbox offers, for "type to add a block".

import * as Blockly from 'blockly/core';

type State = Blockly.serialization.blocks.State;
type Item = Blockly.utils.toolbox.ToolboxItemInfo;

/** How a block plugs in: into a "when" (trigger), a stack (statement), a socket (value), or not at all. */
export type Shape = 'trigger' | 'statement' | 'value' | 'top';

export interface Entry {
  /** What the block says, with its default field values ("turn on light.kitchen"). */
  label: string;
  /** Category path ("Home Assistant › Light"). */
  path: string;
  state: State;
  haystack: string;
  shape?: Shape;
  /** Output check of a value block (null: fits any socket). */
  output?: string[] | null;
  /** Higher ranks first: hand-made blocks above generated ones, technical ones last. */
  tier?: number;
}

/** What the selected block can take next, for ranking. */
export interface Want {
  shape: Exclude<Shape, 'top'>;
  /** Check of the empty socket a value would go into (null: anything; undefined: no socket). */
  check?: string[] | null;
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

const CURATED = new Set(['Events', 'Control', 'Sensing', 'Home', 'Lights & colours', 'Logic', 'Math', 'Text', 'Lists', 'Time', 'Variables', 'Stored', 'Functions']);

export function tierOf(type: string, path: string): number {
  if (/^ha_(svc|trg|cnd)__/.test(type)) return 1;
  const top = path.split(' › ')[0];
  if (top === 'Advanced' || /yaml|_raw$/.test(type)) return 0;
  return CURATED.has(top) ? 2 : 1;
}

export function isTrigger(b: Blockly.Block): boolean {
  return !!b.previousConnection?.getCheck()?.includes('Trigger');
}

export function shapeOf(b: Blockly.Block): Shape {
  if (b.outputConnection) return 'value';
  if (isTrigger(b)) return 'trigger';
  return b.previousConnection ? 'statement' : 'top';
}

// Template code, and the "＋ option" menus of generated blocks (they're not what the block does).
const hidden = (t: string) => /\{[{%]/.test(t) || /^[＋+] ?(option|more settings)$/.test(t);

/** The words a block shows, leaving out hidden fields and template code. */
export function blockWords(b: Blockly.Block, opts: { header?: boolean } = {}): string {
  const out: string[] = [];
  for (const input of b.inputList) {
    if (!input.isVisible()) continue;
    if (opts.header && input.type === Blockly.inputs.inputTypes.STATEMENT) break;
    for (const f of input.fieldRow) {
      if (!f.isVisible()) continue;
      const t = (f.getText() ?? '').trim();
      if (t && !hidden(t)) out.push(t === 'choose…' ? '…' : t);
    }
    if (input.type !== Blockly.inputs.inputTypes.VALUE) continue;
    const child = input.connection?.targetBlock();
    out.push(child ? blockWords(child) || '…' : '…');
  }
  return out.join(' ').replace(/\s+/g, ' ').trim();
}

let scratch: Blockly.Workspace | null = null;

/** Renders a block's text without showing it (headless workspace). */
function describe(state: State): Omit<Entry, 'path' | 'haystack'> | null {
  scratch ??= new Blockly.Workspace();
  Blockly.Events.disable();
  try {
    const b = Blockly.serialization.blocks.append(state, scratch, { recordUndo: false });
    const label = blockWords(b);
    const shape = shapeOf(b);
    const output = b.outputConnection?.getCheck() ?? null;
    b.dispose(false);
    return { label: label || state.type, state, shape, output };
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
    const d = describe(state);
    if (d) entries.push({ ...d, path, tier: tierOf(state.type, path), haystack: `${d.label} ${path} ${state.type}`.toLowerCase() });
  }
  cache.set(toolbox, entries);
  return entries;
}

/** Variables and functions of the workspace (these categories are built on the fly). */
export function workspaceEntries(ws: Blockly.WorkspaceSvg): Entry[] {
  const out: Entry[] = [];
  const add = (label: string, path: string, state: State, shape: Shape) =>
    out.push({ label, path, state, shape, output: null, tier: 2, haystack: `${label} ${path} ${state.type}`.toLowerCase() });
  for (const v of ws.getVariableMap().getAllVariables()) {
    const name = v.getName();
    const kind = v.getType() === 'Global' ? 'Stored (global)' : v.getType() === 'Program' ? 'Stored' : 'Variables';
    add(name, kind, { type: 'variables_get', fields: { VAR: { id: v.getId() } } }, 'value');
    add(`set ${name} to`, kind, { type: 'variables_set', fields: { VAR: { id: v.getId() } } }, 'statement');
  }
  const [plain, returning] = Blockly.Procedures.allProcedures(ws);
  for (const [name, params] of plain) {
    add(`${name}${params.length ? ' with ' + params.join(', ') : ''}`, 'Functions', {
      type: 'procedures_callnoreturn',
      extraState: { name, params },
    }, 'statement');
  }
  for (const [name, params] of returning) {
    add(`${name}${params.length ? ' with ' + params.join(', ') : ''}`, 'Functions', {
      type: 'procedures_callreturn',
      extraState: { name, params },
    }, 'value');
  }
  return out;
}

/** Other words people use for what a block is about (query word → words the blocks use). */
const SYNONYMS: Record<string, string[]> = {
  motion: ['becomes', 'changes', 'occupancy', 'presence', 'movement', 'detected'],
  movement: ['motion', 'occupancy', 'becomes'],
  presence: ['occupancy', 'motion', 'home', 'person', 'zone'],
  occupancy: ['motion', 'presence', 'becomes'],
  notify: ['notification', 'message', 'alert'],
  notification: ['notify', 'message', 'send'],
  message: ['notify', 'notification', 'send', 'say', 'log'],
  alert: ['notify', 'notification', 'message'],
  push: ['notify', 'notification'],
  lamp: ['light'],
  bulb: ['light'],
  light: ['lamp', 'brightness', 'colour'],
  dim: ['brightness', 'light'],
  brightness: ['dim', 'light'],
  color: ['colour'],
  plug: ['switch', 'outlet'],
  socket: ['switch', 'plug'],
  outlet: ['switch', 'plug'],
  delay: ['wait', 'pause'],
  pause: ['wait', 'delay'],
  sleep: ['wait', 'delay'],
  timer: ['wait', 'after', 'delay'],
  speak: ['say', 'tts', 'announce'],
  announce: ['say', 'tts', 'speak'],
  tts: ['say', 'speak'],
  temp: ['temperature', 'climate', 'thermostat'],
  temperature: ['temp', 'climate', 'thermostat', 'above', 'below'],
  thermostat: ['climate', 'temperature', 'heat'],
  heating: ['climate', 'thermostat', 'heat'],
  door: ['contact', 'opening', 'becomes', 'open'],
  window: ['contact', 'opening', 'becomes', 'open'],
  blind: ['cover', 'shutter', 'curtain'],
  shutter: ['cover', 'blind'],
  curtain: ['cover', 'blind'],
  sunrise: ['sun'],
  sunset: ['sun'],
  schedule: ['time', 'clock', 'every', 'cron'],
  clock: ['time', 'every'],
  loop: ['repeat', 'forever', 'while', 'each'],
  forever: ['repeat', 'loop'],
  if: ['condition', 'else'],
  condition: ['if', 'check', 'is'],
  variable: ['set', 'stored', 'var'],
  print: ['log'],
  string: ['text'],
  array: ['list'],
  arrive: ['zone', 'home', 'person', 'presence'],
  leave: ['zone', 'person', 'away', 'presence'],
  away: ['zone', 'person', 'not_home', 'presence'],
};

/** The query word and the words it may stand for (a word typed in full or in part, like "noti"). */
function alternatives(w: string): string[] {
  const alts = new Set<string>();
  for (const [k, syn] of Object.entries(SYNONYMS)) {
    if (k === w || (w.length >= 3 && k.startsWith(w))) syn.forEach((s) => alts.add(s));
  }
  alts.delete(w);
  return [...alts];
}

function wordScore(label: string, w: string): number {
  const at = label.indexOf(w);
  if (at < 0) return 0;
  // A whole word ("turn") beats the start of a longer one ("turned").
  const whole = !/\w/.test(label[at + w.length] ?? ' ') ? 1 : 0;
  if (at === 0) return 6 + whole;
  if (/\W/.test(label[at - 1])) return 4 + whole;
  return 2;
}

const overlaps = (a: string[] | null | undefined, b: string[] | null | undefined) =>
  !a || !b || a.some((x) => b.includes(x));

function fitScore(e: Entry, want: Want | null | undefined): number {
  if (!want || !e.shape) return 0;
  // With a "when" or trigger selected, an action fits too: it goes into the "do".
  if (want.shape === 'trigger' && e.shape === 'statement') return 3;
  if (e.shape === 'value') {
    if (want.check === undefined || !overlaps(e.output, want.check)) return 0;
    return want.shape === 'value' ? 10 : 4;
  }
  return e.shape === want.shape ? 10 : 0;
}

/**
 * Every query word (or a synonym of it) must appear. Matches at the start of the label's words
 * rank higher, then hand-made blocks above generated and technical ones, and blocks that fit
 * the selected block above the rest.
 */
export function search(entries: Entry[], query: string, limit = 40, want?: Want | null): Entry[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return [];
  const alts = words.map(alternatives);
  const scored: [number, Entry][] = [];
  for (const e of entries) {
    let score = 0;
    let ok = true;
    const label = e.label.toLowerCase();
    for (let i = 0; i < words.length && ok; i++) {
      const w = words[i];
      if (e.haystack.includes(w)) {
        score += wordScore(label, w) || 1;
        continue;
      }
      // Synonyms only count at the start of a word of the label or category ("lamp" → "Lights & colours"),
      // not inside type names, where "state" would match half the blocks for "motion".
      const path = e.path.toLowerCase();
      if (alts[i].some((a) => wordScore(label, a) >= 4)) score += 3;
      else if (alts[i].some((a) => wordScore(path, a) >= 4)) score += 1;
      else ok = false;
    }
    if (!ok) continue;
    if (words.length > 1 && label.includes(words.join(' '))) score += 4;
    score += 10 * (e.tier ?? 1) + fitScore(e, want);
    scored.push([score - label.length / 100, e]);
  }
  return scored
    .sort((a, b) => b[0] - a[0])
    .slice(0, limit)
    .map(([, e]) => e);
}

/** Usable selection: a block of this workspace, not in the flyout and not a shadow. */
export function selectedBlock(ws: Blockly.WorkspaceSvg): Blockly.BlockSvg | null {
  const sel = Blockly.getSelected();
  if (!(sel instanceof Blockly.BlockSvg) || sel.isInFlyout || sel.workspace !== ws) return null;
  let s: Blockly.BlockSvg = sel;
  while (s.isShadow() && s.getParent()) s = s.getParent()!;
  return s;
}

/** A block with a statement input for triggers, like "when". */
function triggerInput(b: Blockly.Block): Blockly.Input | null {
  return b.inputList.find((i) => i.type === Blockly.inputs.inputTypes.STATEMENT && i.connection?.getCheck()?.includes('Trigger')) ?? null;
}

/** The "when" (or other trigger holder) around a trigger block. */
function triggerHost(b: Blockly.Block): Blockly.Block | null {
  for (let p = b.getSurroundParent(); p; p = p.getSurroundParent()) if (triggerInput(p)) return p;
  return null;
}

const isEmpty = (c: Blockly.Connection) => !c.targetBlock() || c.targetBlock()!.isShadow();

function firstEmptyValueInput(b: Blockly.Block): Blockly.Input | null {
  return b.inputList.find((i) => i.isVisible() && i.type === Blockly.inputs.inputTypes.VALUE && i.connection && isEmpty(i.connection)) ?? null;
}

/** What fits the selected block best, for ranking the search results. */
export function wantFor(target: Blockly.Block | null): Want | null {
  if (!target) return null;
  const triggers = triggerInput(target);
  if (triggers) return { shape: triggers.connection!.targetBlock() ? 'statement' : 'trigger' };
  // A value fits the selected block's first empty socket, if it has one.
  const input = firstEmptyValueInput(target);
  const check = input ? input.connection!.getCheck() : undefined;
  if (isTrigger(target)) return { shape: 'trigger', check };
  if (target.outputConnection) return input ? { shape: 'value', check } : null;
  return { shape: 'statement', check };
}

/** Creates the block, attached to the selected block (`target`) when it fits, else at `at`. */
export function insert(ws: Blockly.WorkspaceSvg, state: State, at: Blockly.utils.Coordinate, target: Blockly.BlockSvg | null) {
  Blockly.Events.setGroup(true);
  try {
    const block = Blockly.serialization.blocks.append(state, ws) as Blockly.BlockSvg;
    const live = target && !target.isDeadOrDying() ? target : null;
    if (!((live && attach(live, block)) || (!live && isTrigger(block) && intoEmptyWhen(ws, block)))) block.moveTo(at);
    // Through the focus manager, so the block selected before (and focused, passively, while
    // the search box had focus) is deselected, and keyboard shortcuts work on the new block.
    Blockly.getFocusManager().focusNode(block);
    ws.scrollBoundsIntoView(block.getBoundingRectangleWithoutChildren());
    return block;
  } finally {
    Blockly.Events.setGroup(false);
  }
}

function tryConnect(a: Blockly.Connection | null | undefined, b: Blockly.Connection | null | undefined): boolean {
  if (!a || !b) return false;
  const checker = a.getSourceBlock().workspace.connectionChecker;
  if (!checker.canConnect(a, b, false)) return false;
  a.connect(b);
  return true;
}

/** Puts `block` at the end of the stack in `input` (or into it, when it's empty). */
function append(input: Blockly.Input | null, block: Blockly.Block): boolean {
  const c = input?.connection;
  if (!c) return false;
  let last = c.targetBlock();
  if (!last || last.isShadow()) return tryConnect(c, block.previousConnection);
  while (last.getNextBlock()) last = last.getNextBlock()!;
  return tryConnect(last.nextConnection, block.previousConnection);
}

export function intoEmptyWhen(ws: Blockly.Workspace, block: Blockly.Block): boolean {
  for (const top of ws.getTopBlocks(true)) {
    const input = triggerInput(top);
    if (input && top.isEnabled() && !input.connection?.targetBlock()) return append(input, block);
  }
  return false;
}

/** Connects `block` where it fits the selected `target` (see `insert`); false if it doesn't. */
export function attach(target: Blockly.Block, block: Blockly.Block): boolean {
  const host = triggerInput(target) ? target : isTrigger(target) ? triggerHost(target) : null;
  if (isTrigger(block)) {
    if (isTrigger(target) && tryConnect(target.nextConnection, block.previousConnection)) return true;
    return !!host && append(triggerInput(host), block);
  }
  if (block.previousConnection && host) {
    // An action picked while a "when" or one of its triggers is selected goes at the end of its "do".
    const body = host.getInput('DO');
    if (body) return append(body, block);
    return tryConnect(host.nextConnection, block.previousConnection);
  }
  if (block.outputConnection) {
    for (const input of target.inputList) {
      const c = input.connection;
      if (!input.isVisible() || input.type !== Blockly.inputs.inputTypes.VALUE || !c || !isEmpty(c)) continue;
      if (tryConnect(c, block.outputConnection)) return true;
    }
    return false;
  }
  if (tryConnect(target.nextConnection, block.previousConnection)) return true;
  for (const input of target.inputList) {
    const c = input.connection;
    if (input.type === Blockly.inputs.inputTypes.STATEMENT && c && !c.targetBlock() && tryConnect(c, block.previousConnection)) return true;
  }
  return false;
}
