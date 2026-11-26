//! "Switch to …": replace a block with a related one, keeping its id, fields, inner blocks
//! and position in the stack (turn on → turn off, state of → number state of, …).

import * as Blockly from 'blockly/core';
import { allBlockDefs, blockDef } from '../blocks/dynamic';
import { deviceAlternatives } from '../blocks/devices';

export interface Alternative {
  label: string;
  type: string;
  extraState?: unknown;
}

/** Built-in blocks that can stand in for each other (same shape, mostly the same fields). */
const GROUPS: [string, string][][] = [
  [
    ['light_set_colour', 'set to colour'],
    ['light_set_white', 'set to white'],
    ['light_set_brightness', 'set brightness'],
  ],
  [
    ['colour_picker', 'colour picker'],
    ['colour_rgb255', 'colour from RGB'],
    ['colour_hsv', 'colour from hue'],
    ['colour_random', 'random colour'],
  ],
  [
    ['ha_trigger_state', 'state changes'],
    ['ha_trigger_numeric', 'goes above / below'],
  ],
  [
    ['ha_trigger_time', 'time is'],
    ['ha_trigger_every', 'every N minutes'],
    ['ha_trigger_sun', 'sunrise / sunset'],
    ['engine_trigger_cron', 'schedule (cron)'],
  ],
  [
    ['ha_state', 'state of'],
    ['ha_number_state', 'number state of'],
    ['ha_attribute', 'attribute of'],
    ['ha_friendly_name', 'name of'],
    ['ha_seconds_since', 'seconds since'],
  ],
  [
    ['ha_is_state', 'is (state)'],
    ['ha_state_for', 'has been … for'],
    ['ha_has_value', 'has a value'],
  ],
  [
    ['ha_entity', 'entity'],
    ['ha_area', 'area'],
    ['ha_floor', 'floor'],
    ['ha_label', 'label'],
    ['ha_device', 'device'],
  ],
  [
    ['ha_state_check', 'any / all of … are'],
    ['ha_states_aggregate', 'average / min / max of states'],
  ],
  [
    ['ha_wait', 'wait N seconds'],
    ['wait_until', 'wait until'],
  ],
  [
    ['time_now', 'now'],
    ['time_today_at', 'today at'],
  ],
];

export function alternatives(block: Blockly.Block): Alternative[] {
  if (block.type.startsWith('device_')) {
    return deviceAlternatives(block).map((a) => ({ label: a.label, type: block.type, extraState: a.extraState }));
  }
  const def = blockDef(block.type);
  if (def) {
    // Generated blocks: same category and kind, e.g. the other light actions.
    const path = def.category.join('/');
    return [...allBlockDefs()]
      .filter((d) => d.type !== def.type && d.kind === def.kind && d.category.join('/') === path && !!d.target === !!def.target)
      .sort((a, b) => a.title.localeCompare(b.title))
      .map((d) => ({ label: d.title, type: d.type }));
  }
  const group = GROUPS.find((g) => g.some(([t]) => t === block.type));
  return (group ?? []).filter(([t]) => t !== block.type && Blockly.Blocks[t]).map(([type, label]) => ({ label, type }));
}

/** Keeps the optional params of a generated block that the new type also has. */
function carriedExtraState(block: Blockly.Block, type: string): unknown {
  const extra = block.saveExtraState?.() as { optional?: string[] } | null | undefined;
  const def = blockDef(type);
  if (!def || !extra?.optional) return undefined;
  const names = new Set(def.params.map((p) => p.name));
  const optional = extra.optional.filter((n) => names.has(n));
  return optional.length ? { optional } : undefined;
}

export function switchBlock(old: Blockly.BlockSvg, alt: Alternative): Blockly.BlockSvg {
  const ws = old.workspace;
  Blockly.Events.setGroup(true);
  try {
    const id = old.id;
    const xy = old.getRelativeToSurfaceXY();
    const parent = old.outputConnection?.targetConnection ?? old.previousConnection?.targetConnection ?? null;
    const fields = new Map<string, unknown>();
    const children: { name: string; child: Blockly.BlockSvg | null; shadow: Blockly.serialization.blocks.State | null }[] = [];
    for (const input of old.inputList) {
      for (const f of input.fieldRow) if (f.name && f.EDITABLE) fields.set(f.name, f.getValue());
      const c = input.connection;
      if (!c) continue;
      const child = c.targetBlock() as Blockly.BlockSvg | null;
      children.push({ name: input.name, child: child && !child.isShadow() ? child : null, shadow: c.getShadowState(true) });
    }
    const next = old.getNextBlock() as Blockly.BlockSvg | null;
    const extraState = alt.extraState ?? carriedExtraState(old, alt.type);
    for (const { child } of children) child?.unplug(false);
    next?.unplug(false);
    old.unplug(false);
    old.dispose(false);

    const block = Blockly.serialization.blocks.append(
      { type: alt.type, id, ...(extraState ? { extraState } : {}) },
      ws,
    ) as Blockly.BlockSvg;
    for (const [name, value] of fields) {
      const f = block.getField(name);
      if (!f) continue;
      try {
        f.setValue(value);
      } catch {
        /* the new block doesn't accept that value; keep its default */
      }
    }
    const checker = ws.connectionChecker;
    const link = (a: Blockly.Connection | null | undefined, b: Blockly.Connection | null | undefined) =>
      !!a && !!b && checker.canConnect(a, b, false) && (a.connect(b), true);
    for (const { name, child, shadow } of children) {
      const c = block.getInput(name)?.connection;
      if (!c) continue;
      if (shadow) c.setShadowState(shadow);
      if (child) link(c, child.outputConnection ?? child.previousConnection);
    }
    if (next) link(block.nextConnection, next.previousConnection);
    if (!(parent && link(parent, block.outputConnection ?? block.previousConnection))) block.moveTo(xy);
    block.select();
    return block;
  } finally {
    Blockly.Events.setGroup(false);
  }
}

const SLOTS = 10;

/** Context-menu entries "Switch to: …" (Blockly menus are flat, so one slot per option). */
export function registerSwitchMenu() {
  for (let i = 0; i < SLOTS; i++) {
    Blockly.ContextMenuRegistry.registry.register({
      id: `puzzlebox_switch_${i}`,
      scopeType: Blockly.ContextMenuRegistry.ScopeType.BLOCK,
      weight: 20 + i,
      displayText: (scope) => `Switch to: ${alternatives(scope.block!)[i]?.label ?? ''}`,
      preconditionFn: (scope) => {
        const b = scope.block;
        if (!b || b.isInFlyout || b.isShadow() || !b.isEditable()) return 'hidden';
        return alternatives(b)[i] ? 'enabled' : 'hidden';
      },
      callback: (scope) => {
        const alt = alternatives(scope.block!)[i];
        if (alt) switchBlock(scope.block as Blockly.BlockSvg, alt);
      },
    });
  }
}
