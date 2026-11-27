//! Editor-side checks the compiler can't make because they need Home Assistant's entities:
//! warnings, not errors, so the program still saves.

import * as Blockly from 'blockly/core';
import { FieldEntity, entityState, knownEntities } from './field_entity';

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
  return [...out, ...sceneWarnings(ws)];
}

/** The scene id "remember … as" uses for a name, as the compiler makes it (`scene_slug`). */
export function sceneSlug(name: string): string {
  return name
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '_')
    .replace(/^_+|_+$/g, '');
}

/** "remember … as" names whose scene is the user's own: HA keeps that one and ignores the
 * snapshot. Mirrors the engine's `scene_taken`. */
function sceneWarnings(ws: Blockly.Workspace): BlockWarning[] {
  const out: BlockWarning[] = [];
  for (const b of ws.getBlocksByType('scene_remember', false)) {
    if (!b.isEnabled()) continue;
    const id = sceneSlug(String(b.getFieldValue('NAME') ?? ''));
    const s = id && entityState(`scene.${id}`);
    if (!s) continue;
    const a = s.attributes;
    if ('id' in a || !Array.isArray(a.entity_id) || a.friendly_name !== id) {
      out.push({
        block_id: b.id,
        message: `There's already a scene called scene.${id} in your configuration: choose another name`,
      });
    }
  }
  return out;
}
