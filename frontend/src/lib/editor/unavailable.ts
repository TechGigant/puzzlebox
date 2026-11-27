//! Blocks the editor can't show, because the integration, Zigbee2MQTT or plugin that provides
//! them isn't connected (yet) or is gone, load as placeholders. A placeholder keeps the block
//! exactly as it was saved, with everything inside it, and saves it back unchanged, so opening
//! and saving a program never loses blocks. Blocks after it stay real blocks on the canvas.

import * as Blockly from 'blockly/core';

export const UNAVAILABLE = 'pb_unavailable';

type Slot = { block?: Node; shadow?: Node };
type Node = { type: string; id?: string; x?: number; y?: number; next?: Slot; inputs?: Record<string, Slot>; [k: string]: unknown };
type Shape = 'statement' | 'value' | 'top';

interface Extra {
  /** The block as saved, without `next`. */
  original: Node;
  /** Its keys in saved order, so it saves back exactly as it was. */
  keys: string[];
  shape: Shape;
  label: string;
  reason: string;
}

/** Why a block type is missing; `pending` when it should turn up once something connects. */
export interface Why {
  reason: string;
  pending: boolean;
}

export interface Missing extends Why {
  type: string;
  id: string;
  label: string;
}

type Placeholder = Blockly.Block & { pbExtra?: Extra };

/** What's inside a block (its inputs), not counting the default values in sockets. */
function blocksInside(node: Node): number {
  let n = 0;
  for (const slot of Object.values(node.inputs ?? {})) {
    if (slot.block) n += 1 + blocksInside(slot.block) + chainLength(slot.block.next?.block);
  }
  return n;
}

function chainLength(node: Node | undefined): number {
  let n = 0;
  for (let b = node; b; b = b.next?.block) n += 1 + blocksInside(b);
  return n;
}

export function registerUnavailableBlock() {
  Blockly.Blocks[UNAVAILABLE] = {
    init(this: Placeholder) {
      this.appendDummyInput('TITLE').appendField(new Blockly.FieldLabel(''), 'TITLE');
      this.appendDummyInput('WHY').appendField(new Blockly.FieldLabel(''), 'WHY');
      this.setColour('#8d8d8d');
    },
    saveExtraState(this: Placeholder) {
      return this.pbExtra ?? null;
    },
    loadExtraState(this: Placeholder, state: Extra) {
      this.pbExtra = state;
      if (state.shape === 'value') this.setOutput(true);
      if (state.shape === 'statement') {
        this.setPreviousStatement(true);
        this.setNextStatement(true);
      }
      this.setFieldValue(`‹${state.label}› isn't available right now`, 'TITLE');
      this.setFieldValue(state.reason, 'WHY');
      const inside = blocksInside(state.original);
      const kept = inside ? ` It holds ${inside === 1 ? '1 more block' : `${inside} more blocks`}, kept as they are.` : '';
      this.setTooltip(`${state.reason}${kept} The block is saved exactly as it was, and shows again once it's available.`);
    },
  };
}

export function isPlaceholder(b: Blockly.Block | null | undefined): b is Placeholder {
  return b?.type === UNAVAILABLE;
}

export function placeholderInfo(b: Blockly.Block | null | undefined): { label: string; reason: string } | null {
  return isPlaceholder(b) && b.pbExtra ? { label: b.pbExtra.label, reason: b.pbExtra.reason } : null;
}

let scratch: Blockly.Workspace | null = null;

/** Whether `parent`'s input `name` takes a stack of blocks or a value. */
function inputShape(parent: Node, name: string, child: Node): Shape {
  scratch ??= new Blockly.Workspace();
  Blockly.Events.disable();
  try {
    const { type, extraState, fields } = parent;
    const b = Blockly.serialization.blocks.append({ type, extraState, fields } as Blockly.serialization.blocks.State, scratch, { recordUndo: false });
    const input = b.getInput(name);
    const shape = input?.type === Blockly.inputs.inputTypes.STATEMENT ? 'statement' : input ? 'value' : null;
    b.dispose(false);
    if (shape) return shape;
  } catch {
    /* falls back to a guess */
  } finally {
    Blockly.Events.enable();
  }
  return child.next ? 'statement' : 'value';
}

/** "ha_svc__light__turn_on" → "Turn on light", for blocks the editor has no text for. */
export function typeLabel(type: string): string {
  const parts = type.split('__');
  const words = (s: string) => s.replace(/_/g, ' ').trim();
  let text: string;
  if (parts[0] === 'ha_svc' && parts.length >= 3) text = `${words(parts[2])} ${words(parts[1])}`;
  else if (parts.length >= 3) text = `${words(parts[parts.length - 2])} ${words(parts[parts.length - 1])}`;
  else text = words(parts[parts.length - 1]);
  return text.charAt(0).toUpperCase() + text.slice(1);
}

/** Replaces blocks of unknown types (`known` false) by placeholders; changes `state` in place. */
export function wrapUnavailable(
  state: object,
  known: (type: string) => boolean,
  why: (type: string) => Why,
  label: (node: Node) => string,
): Missing[] {
  const missing: Missing[] = [];
  const wrap = (node: Node, shape: Shape): Node => {
    if (!known(node.type)) {
      const { next, ...original } = node;
      const w = why(node.type);
      const extra: Extra = { original, keys: Object.keys(node), shape: shape === 'top' && next ? 'statement' : shape, label: label(node), reason: w.reason };
      missing.push({ type: node.type, id: node.id ?? '', label: extra.label, ...w });
      const ph: Node = { type: UNAVAILABLE };
      if (node.id !== undefined) ph.id = node.id;
      if (node.x !== undefined) ph.x = node.x;
      if (node.y !== undefined) ph.y = node.y;
      ph.extraState = extra;
      if (next?.block) ph.next = { block: wrap(next.block, 'statement') };
      return ph;
    }
    for (const [name, slot] of Object.entries(node.inputs ?? {})) {
      if (slot.block) slot.block = wrap(slot.block, known(slot.block.type) ? 'value' : inputShape(node, name, slot.block));
      if (slot.shadow) slot.shadow = wrap(slot.shadow, known(slot.shadow.type) ? 'value' : inputShape(node, name, slot.shadow));
    }
    if (node.next?.block) node.next.block = wrap(node.next.block, 'statement');
    return node;
  };
  const top = (state as { blocks?: { blocks?: Node[] } }).blocks?.blocks;
  if (Array.isArray(top)) top.forEach((b, i) => (top[i] = wrap(b, 'top')));
  return missing;
}

/** Turns placeholders back into the blocks they keep; changes `state` in place. */
export function unwrapUnavailable<T extends object>(state: T): T {
  const unwrap = (node: Node): Node => {
    if (node.type === UNAVAILABLE && node.extraState) {
      const extra = node.extraState as Extra;
      const keys = [...extra.keys];
      // Moved out of a stack onto the canvas: it now has a position, which goes after its id.
      if (node.x !== undefined && !keys.includes('x')) keys.splice(keys.indexOf('id') + 1, 0, 'x', 'y');
      if (node.next?.block && !keys.includes('next')) keys.push('next');
      const out: Node = { type: extra.original.type };
      for (const k of keys) {
        if (k === 'next') {
          if (node.next?.block) out.next = { block: unwrap(node.next.block) };
        } else if (k === 'x' || k === 'y' || k === 'id') {
          // A pasted copy has a new id.
          if (node[k] !== undefined) (out as Record<string, unknown>)[k] = node[k];
        } else if (k in extra.original) {
          out[k] = extra.original[k];
        }
      }
      return out;
    }
    for (const slot of Object.values(node.inputs ?? {})) {
      if (slot.block) slot.block = unwrap(slot.block);
      if (slot.shadow) slot.shadow = unwrap(slot.shadow);
    }
    if (node.next?.block) node.next.block = unwrap(node.next.block);
    return node;
  };
  const top = (state as { blocks?: { blocks?: Node[] } }).blocks?.blocks;
  if (Array.isArray(top)) top.forEach((b, i) => (top[i] = unwrap(b)));
  return state;
}

/** How often each of `types` appears in a saved workspace. */
export function countTypes(state: unknown, types: Set<string>, out = new Map<string, number>()): Map<string, number> {
  if (!state || typeof state !== 'object') return out;
  if (Array.isArray(state)) {
    for (const x of state) countTypes(x, types, out);
    return out;
  }
  const o = state as Record<string, unknown>;
  if (typeof o.type === 'string' && types.has(o.type)) out.set(o.type, (out.get(o.type) ?? 0) + 1);
  for (const v of Object.values(o)) countTypes(v, types, out);
  return out;
}
