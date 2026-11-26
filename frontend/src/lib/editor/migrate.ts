//! Updates saved workspaces to the current block shapes when a program is opened. The engine
//! still understands the old shapes, so programs keep running until they are opened and saved.

import { blockDef } from '../blocks/dynamic';

type State = {
  type: string;
  fields?: Record<string, unknown>;
  inputs?: Record<string, { block?: State; shadow?: State }>;
  next?: { block?: State };
  extraState?: unknown;
  [k: string]: unknown;
};

/** Blocks whose entity moved from a typed-in field to a socket. */
const ENTITY_SOCKET = new Set([
  'ha_state',
  'ha_attribute',
  'ha_is_state',
  'ha_state_for',
  'ha_has_value',
  'ha_number_state',
  'ha_friendly_name',
  'ha_seconds_since',
  'ha_history',
  'light_colour',
  'light_brightness',
  'cover_position_of',
  'climate_temperature_of',
  'media_volume_of',
  'media_title_of',
  'fan_speed_of',
]);

/** Number fields that became sockets (trigger durations). */
const NUMBER_SOCKET: Record<string, string[]> = {
  ha_trigger_state: ['FOR'],
  ha_trigger_sun: ['OFFSET'],
};

/** Seconds sockets that now show a duration block ("5 minutes") instead of a bare number. */
const SECONDS: Record<string, string[]> = {
  ha_wait: ['SECONDS'],
  ha_wait_for_trigger: ['TIMEOUT'],
  timer_start: ['SECONDS'],
  wait_until: ['HOLD', 'TIMEOUT'],
  throttle: ['SECONDS'],
  ha_state_for: ['SECONDS'],
  ha_history: ['SECONDS'],
};

/** Duration sockets added to older triggers; an empty one compiles like before. */
const NEW_DURATION: Record<string, [string, 'seconds' | 'minutes'][]> = {
  ha_trigger_numeric: [['FOR', 'seconds']],
  ha_trigger_time: [['OFFSET', 'minutes']],
};

const duration = (n: unknown) => ({
  type: 'duration',
  inputs: { N: { shadow: { type: 'math_number', fields: { NUM: n } } } },
  fields: { UNIT: 'seconds' },
});

const num = (n: number) => ({ shadow: { type: 'math_number', fields: { NUM: n } } as State });
const divide = (a: { block?: State; shadow?: State }, b: { block?: State; shadow?: State }): State => ({
  type: 'math_arithmetic',
  fields: { OP: 'DIVIDE' },
  inputs: { A: a, B: b },
});

function migrateBlock(b: State) {
  b.inputs ??= {};
  if (ENTITY_SOCKET.has(b.type) && b.fields && 'ENTITY' in b.fields && !b.inputs.ENTITY) {
    b.inputs.ENTITY = { shadow: { type: 'ha_entity', fields: { ENTITY: b.fields.ENTITY } } };
    delete b.fields.ENTITY;
  }
  for (const name of NUMBER_SOCKET[b.type] ?? []) {
    if (b.fields && name in b.fields && !b.inputs[name]) {
      b.inputs[name] = { shadow: duration(b.fields[name]) as State };
      delete b.fields[name];
    }
  }
  for (const [name, unit] of NEW_DURATION[b.type] ?? []) {
    b.inputs[name] ??= { shadow: { ...duration(0), fields: { UNIT: unit } } as State };
  }
  for (const name of SECONDS[b.type] ?? []) {
    const slot = b.inputs[name];
    if (slot && !slot.block && slot.shadow?.type === 'math_number') {
      slot.shadow = duration(slot.shadow.fields?.NUM) as State;
    }
  }
  // Blocks that returned different kinds of value were split.
  if (b.type === 'ha_state_check' && b.fields?.MODE === 'count') {
    b.type = 'ha_state_count';
    delete b.fields.MODE;
  }
  if (b.type === 'ha_history' && b.fields?.OP === 'state_ago') {
    b.type = 'ha_state_ago';
    delete b.fields.OP;
    delete b.fields.STATE;
  }
  if (b.type === 'trigger_info' && (b.fields?.WHAT === 'id' || b.fields?.WHAT === 'platform')) {
    b.type = 'ha_trigger_data';
    b.fields = { PATH: b.fields.WHAT };
  }
  // Colour settings of generated blocks became sockets.
  const def = blockDef(b.type);
  for (const p of def?.params ?? []) {
    const slot = `P_${p.name}`;
    if (p.input === 'color' && b.fields && typeof b.fields[slot] === 'string' && !b.inputs[slot]) {
      b.inputs[slot] = { shadow: { type: 'colour_picker', fields: { COLOUR: b.fields[slot] } } };
      delete b.fields[slot];
    }
  }
  // Zigbee2MQTT group brightness (0–254) and white (mired) became % and kelvin.
  if (/^z2m__.+__group\d+__set$/.test(b.type)) {
    // [old name, new name, convert a number, block that converts a calculation]
    const convert: [string, string, (n: number) => number, (e: State) => State][] = [
      ['brightness', 'brightness_percent', (n) => Math.round(n / 2.54), (e) => divide({ block: e }, num(2.54))],
      ['color_temp', 'color_temp_kelvin', (n) => Math.round(1_000_000 / Math.max(n, 1)), (e) => divide(num(1_000_000), { block: e })],
    ];
    const extra = b.extraState as { optional?: string[] } | undefined;
    for (const [from, to, fn, wrap] of convert) {
      const slot = b.inputs[`P_${from}`];
      if (!slot) continue;
      if (slot.block) {
        b.inputs[`P_${to}`] = { block: wrap(slot.block) };
      } else {
        const n = Number(slot.shadow?.fields?.NUM);
        b.inputs[`P_${to}`] = { shadow: { type: 'math_number', fields: { NUM: Number.isFinite(n) ? fn(n) : 0 } } };
      }
      delete b.inputs[`P_${from}`];
      if (extra?.optional) extra.optional = extra.optional.map((o) => (o === from ? to : o));
    }
  }
  if (!Object.keys(b.inputs).length) delete b.inputs;
}

function walk(b: State | undefined) {
  if (!b || typeof b !== 'object') return;
  migrateBlock(b);
  for (const slot of Object.values(b.inputs ?? {})) {
    walk(slot.block);
    walk(slot.shadow);
  }
  walk(b.next?.block);
}

/** Migrates a saved workspace in place and returns it. */
export function migrateWorkspace<T extends object>(ws: T): T {
  const blocks = (ws as { blocks?: { blocks?: State[] } }).blocks?.blocks ?? [];
  blocks.forEach(walk);
  return ws;
}
