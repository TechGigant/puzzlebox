//! Editor-side checks the compiler can't make because they need Home Assistant's entities:
//! warnings, not errors, so the program still saves.

import * as Blockly from 'blockly/core';
import { FieldEntity, knownEntities } from './field_entity';

export interface BlockWarning {
  /** The block to show it on: a shadow's parent, since shadows can't be selected. */
  block_id: string;
  message: string;
}

/** Anything HA resolves at run time, or accepts instead of entity ids. */
const isDynamic = (v: string) => v.includes('{{') || v.includes('{%') || v === 'all' || v === 'none';

/** Values of entity pickers that aren't one of HA's entities (typos, removed devices, "hall"). */
export function entityWarnings(ws: Blockly.Workspace): BlockWarning[] {
  const known = knownEntities();
  if (!known.length) return [];
  const ids = new Set(known.map((k) => k.entity_id));
  const out: BlockWarning[] = [];
  for (const b of ws.getAllBlocks(false)) {
    if (!b.isEnabled()) continue;
    for (const input of b.inputList) {
      for (const f of input.fieldRow) {
        if (!(f instanceof FieldEntity) || !f.isVisible()) continue;
        const raw = String(f.getValue() ?? '');
        if (isDynamic(raw)) continue;
        for (const id of raw.split(',').map((x) => x.trim()).filter(Boolean)) {
          if (ids.has(id)) continue;
          const holder = b.isShadow() ? (b.getParent() ?? b) : b;
          const message = /^[a-z_]+\.[a-z0-9_]+$/.test(id)
            ? `${id} doesn't exist in Home Assistant`
            : `"${id}" isn't an entity: choose one from the list`;
          out.push({ block_id: holder.id, message });
        }
      }
    }
  }
  return out;
}
