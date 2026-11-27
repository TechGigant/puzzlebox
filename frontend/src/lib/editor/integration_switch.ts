//! "Switch to" from the general integration trigger/condition block ("Home Assistant trigger
//! light.turned_on …") to the block Home Assistant generated for it, carrying the options over
//! as its settings. Only offered when every option has a place in the generated block.

import type * as Blockly from 'blockly/core';
import { blockDef, durationShadow, secondsParam, type BlockDef, type Param } from '../blocks/dynamic';

type State = Blockly.serialization.blocks.State;

const PREFIX: Record<string, string> = { ha_trigger_integration: 'ha_trg__', ha_condition_integration: 'ha_cnd__' };

/** What the generated block gets: field values, shadow blocks for sockets, optional settings shown. */
export interface Conversion {
  def: BlockDef;
  fields: Record<string, string>;
  shadows: Record<string, State>;
  optional: string[];
}

const sanitize = (s: string) => s.replace(/[^A-Za-z0-9]/g, '_');

/** The generated block type for a trigger or condition name, as the registry names it. */
export function generatedType(type: string, name: string): string | null {
  const prefix = PREFIX[type];
  if (!prefix) return null;
  const dot = name.indexOf('.');
  const [domain, rest] = dot < 0 ? ['homeassistant', name] : [name.slice(0, dot), name.slice(dot + 1)];
  return `${prefix}${sanitize(domain)}__${sanitize(rest)}`;
}

/** The fixed value of a value block (text, numbers, lists, dictionaries…); undefined otherwise. */
function literal(b: Blockly.Block | null): unknown {
  if (!b) return undefined;
  switch (b.type) {
    case 'text':
      return String(b.getFieldValue('TEXT') ?? '');
    case 'math_number':
      return Number(b.getFieldValue('NUM'));
    case 'logic_boolean':
      return b.getFieldValue('BOOL') === 'TRUE';
    case 'logic_null':
      return null;
    case 'yaml_value': {
      const text = String(b.getFieldValue('CONFIG') ?? '').trim();
      if (!text) return null;
      try {
        return JSON.parse(text);
      } catch {
        return undefined;
      }
    }
    case 'lists_create_with': {
      const out: unknown[] = [];
      for (const input of b.inputList) {
        if (!input.connection) continue;
        const target = input.connection.targetBlock();
        if (!target) continue;
        const v = literal(target);
        if (v === undefined) return undefined;
        out.push(v);
      }
      return out;
    }
    case 'dict_create': {
      const out: Record<string, unknown> = {};
      for (let p = b.getInputTargetBlock('ITEMS'); p; p = p.getNextBlock()) {
        if (p.type !== 'dict_pair' || !p.isEnabled()) continue;
        const v = literal(p.getInputTargetBlock('VALUE'));
        if (v === undefined) return undefined;
        out[String(p.getFieldValue('KEY') ?? '')] = v;
      }
      return out;
    }
    default:
      return undefined;
  }
}

/** Seconds of an HA time period ("00:05:00", 300, {minutes: 5}); undefined if it isn't one. */
function seconds(v: unknown): number | undefined {
  if (typeof v === 'number') return v;
  if (typeof v === 'string') {
    const parts = v.split(':').map(Number);
    if (parts.length >= 2 && parts.length <= 3 && parts.every((n) => Number.isFinite(n))) {
      return parts[0] * 3600 + parts[1] * 60 + (parts[2] ?? 0);
    }
    return undefined;
  }
  if (v && typeof v === 'object' && !Array.isArray(v)) {
    const f: Record<string, number> = { days: 86400, hours: 3600, minutes: 60, seconds: 1, milliseconds: 0.001 };
    let total = 0;
    for (const [k, n] of Object.entries(v)) {
      if (!(k in f) || typeof n !== 'number') return undefined;
      total += n * f[k];
    }
    return total;
  }
  return undefined;
}

const strings = (v: unknown): string | undefined =>
  typeof v === 'string' ? v : Array.isArray(v) && v.every((x) => typeof x === 'string') ? v.join(', ') : undefined;
const shadow = (type: string, fields: Record<string, unknown>): State => ({ type, fields });

/** Puts one option into the generated block's setting; false if it doesn't fit. */
function place(p: Param, v: unknown, c: Conversion): boolean {
  const slot = `P_${p.name}`;
  switch (p.input) {
    case 'select': {
      const s = p.multiple ? strings(v) : typeof v === 'string' ? v : undefined;
      const ok = s !== undefined && (p.multiple || p.options.some((o) => o.value === s));
      if (ok) c.fields[slot] = s;
      return ok;
    }
    case 'boolean':
      if (typeof v !== 'boolean') return false;
      c.fields[slot] = v ? 'TRUE' : 'FALSE';
      return true;
    case 'registry':
    case 'state_value': {
      const s = strings(v);
      if (s === undefined || (!p.multiple && typeof v !== 'string')) return false;
      c.fields[slot] = s;
      return true;
    }
    case 'number':
      if (typeof v !== 'number') return false;
      c.shadows[slot] = secondsParam(p) ? durationShadow(v) : shadow('math_number', { NUM: v });
      return true;
    case 'duration': {
      const s = seconds(v);
      if (s === undefined) return false;
      c.shadows[slot] = durationShadow(s);
      return true;
    }
    case 'text':
    case 'time':
      if (typeof v !== 'string') return false;
      c.shadows[slot] = shadow('text', { TEXT: v });
      return true;
    case 'entity': {
      const s = strings(v);
      if (s === undefined) return false;
      c.shadows[slot] = shadow('ha_entity', { ENTITY: s });
      return true;
    }
    case 'any':
      c.shadows[slot] = shadow('yaml_value', { CONFIG: JSON.stringify(v) });
      return true;
    case 'threshold': {
      const t = v as { type?: unknown; value?: unknown; value_min?: unknown; value_max?: unknown } | null;
      if (!t || typeof t !== 'object' || typeof t.type !== 'string') return false;
      if (t.type === 'any') {
        if (p.mode !== 'changed' || Object.keys(t).length !== 1) return false;
        c.fields[slot] = 'any';
        return true;
      }
      const two = t.type === 'between' || t.type === 'outside';
      if (!['above', 'below', 'between', 'outside'].includes(t.type)) return false;
      const ends = two ? [t.value_min, t.value_max] : [t.value];
      if (Object.keys(t).length !== ends.length + 1) return false;
      let unit: string | null | undefined;
      for (const [i, end] of ends.entries()) {
        const e = end as { number?: unknown; unit_of_measurement?: unknown } | null;
        if (!e || typeof e !== 'object' || typeof e.number !== 'number') return false;
        if (Object.keys(e).some((k) => k !== 'number' && k !== 'unit_of_measurement')) return false;
        const u = (e.unit_of_measurement ?? null) as string | null;
        if (unit !== undefined && u !== unit) return false;
        unit = u;
        c.shadows[`${slot}__${i ? 'b' : 'a'}`] = shadow('math_number', { NUM: e.number });
      }
      if (p.units.length) {
        if (!p.units.includes(unit ?? null)) return false;
        c.fields[`${slot}__unit`] = unit ?? '__none__';
      } else if (unit != null) {
        return false;
      }
      c.fields[slot] = t.type;
      return true;
    }
    default:
      return false;
  }
}

/** How a general integration block becomes its generated block; null if it can't exactly. */
export function conversion(block: Blockly.Block): Conversion | null {
  const type = generatedType(block.type, String(block.getFieldValue('NAME') ?? '').trim());
  const def = type ? blockDef(type) : undefined;
  if (!def) return null;
  const input = block.getInputTargetBlock('OPTIONS');
  const options = input ? literal(input) : null;
  if (options !== null && (typeof options !== 'object' || Array.isArray(options))) return null;
  // The generated block always needs its target when it has one, and has none otherwise.
  const target = block.getInputTargetBlock('TARGET');
  const emptyTarget = !target || (target.type === 'ha_entity' && !String(target.getFieldValue('ENTITY') ?? '').trim());
  if (!def.target !== emptyTarget) return null;
  const c: Conversion = { def, fields: {}, shadows: {}, optional: [] };
  for (const [k, v] of Object.entries((options ?? {}) as Record<string, unknown>)) {
    const p = def.params.find((x) => x.name === k);
    if (!p || !place(p, v, c)) return null;
    if (!p.required) c.optional.push(p.name);
  }
  return c;
}

/** Fills the new block from the conversion (after it was created with `{optional}`). */
export function applyConversion(block: Blockly.Block, c: Conversion) {
  for (const [name, v] of Object.entries(c.fields)) {
    try {
      block.getField(name)?.setValue(v);
    } catch {
      /* keep the block's default */
    }
  }
  for (const [name, s] of Object.entries(c.shadows)) {
    const conn = block.getInput(name)?.connection;
    if (!conn) continue;
    conn.targetBlock()?.dispose(false);
    conn.setShadowState(s);
  }
}
