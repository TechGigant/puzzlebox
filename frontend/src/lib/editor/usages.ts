//! "Find usages" and "Rename everywhere" for variables, timers, entities, functions and
//! areas/floors/labels/devices.

import * as Blockly from 'blockly/core';
import { FieldEntity, FieldRegistry } from '../blocks/field_entity';

interface Ref {
  kind: 'variable' | 'timer' | 'entity' | 'function' | 'registry';
  /** Value as stored (variable id, timer name, entity id, …). */
  value: string;
  /** What users see ("timer lights off", "light.kitchen"). */
  label: string;
}

interface Hit {
  block: Blockly.BlockSvg;
  field: Blockly.Field | null;
}

const TIMER_BLOCKS = new Set(['timer_start', 'timer_cancel', 'timer_running']);
const split = (v: unknown) =>
  String(v ?? '')
    .split(',')
    .map((s) => s.trim())
    .filter(Boolean);

/** References a single block makes, with the field holding each. */
function refsOf(block: Blockly.Block): [Ref, Blockly.Field | null][] {
  const out: [Ref, Blockly.Field | null][] = [];
  for (const input of block.inputList) {
    for (const f of input.fieldRow) {
      if (f instanceof Blockly.FieldVariable) {
        const v = f.getVariable();
        if (v) out.push([{ kind: 'variable', value: v.getId(), label: `variable ${v.getName()}` }, f]);
      } else if (f instanceof FieldEntity || (f.name === 'ENTITY' && f instanceof Blockly.FieldTextInput)) {
        for (const e of split(f.getValue())) out.push([{ kind: 'entity', value: e, label: e }, f]);
      } else if (f instanceof FieldRegistry) {
        for (const id of split(f.getValue())) out.push([{ kind: 'registry', value: id, label: f.getText() || id }, f]);
      } else if (f.name === 'NAME' && TIMER_BLOCKS.has(block.type)) {
        const name = String(f.getValue() ?? '').trim();
        if (name) out.push([{ kind: 'timer', value: name, label: `timer ${name}` }, f]);
      }
    }
  }
  if (block.type.startsWith('procedures_')) {
    const name =
      (block as unknown as { getProcedureCall?: () => string }).getProcedureCall?.() ?? block.getFieldValue('NAME');
    if (name) out.push([{ kind: 'function', value: String(name), label: `function ${name}` }, null]);
  }
  return out;
}

export function primaryRef(block: Blockly.Block): Ref | null {
  return refsOf(block)[0]?.[0] ?? null;
}

export function usages(ws: Blockly.WorkspaceSvg, ref: Ref): Hit[] {
  const hits: Hit[] = [];
  for (const b of ws.getAllBlocks(true) as Blockly.BlockSvg[]) {
    for (const [r, field] of refsOf(b)) {
      if (r.kind === ref.kind && r.value === ref.value) hits.push({ block: b, field });
    }
  }
  return hits;
}

let cursor = { key: '', index: -1 };

/** Highlights every usage and centres on the next one (repeat to step through them). */
export function showNext(ws: Blockly.WorkspaceSvg, ref: Ref) {
  const hits = usages(ws, ref);
  if (!hits.length) return;
  const key = `${ref.kind}:${ref.value}`;
  cursor = { key, index: cursor.key === key ? (cursor.index + 1) % hits.length : 0 };
  for (const h of hits) {
    const el = h.block.getSvgRoot();
    el.classList.add('pb-usage');
    setTimeout(() => el.classList.remove('pb-usage'), 2500);
  }
  const target = hits[cursor.index].block;
  ws.centerOnBlock(target.id);
  target.select();
}

function renameIn(ws: Blockly.WorkspaceSvg, ref: Ref, to: string) {
  Blockly.Events.setGroup(true);
  try {
    for (const { field } of usages(ws, ref)) {
      if (!field) continue;
      const parts = split(field.getValue());
      const next = parts.length > 1 ? parts.map((p) => (p === ref.value ? to : p)).join(', ') : to;
      field.setValue(next);
    }
  } finally {
    Blockly.Events.setGroup(false);
  }
}

export function renameEverywhere(ws: Blockly.WorkspaceSvg, ref: Ref) {
  if (ref.kind === 'variable') {
    const v = ws.getVariableMap().getVariableById(ref.value);
    if (!v) return;
    Blockly.dialog.prompt(`Rename variable "${v.getName()}" everywhere to:`, v.getName(), (to) => {
      if (to?.trim()) ws.getVariableMap().renameVariable(v, to.trim());
    });
    return;
  }
  const what = ref.kind === 'timer' ? `timer "${ref.value}"` : `"${ref.value}"`;
  Blockly.dialog.prompt(`Replace ${what} in every block of this program with:`, ref.value, (to) => {
    if (to?.trim() && to.trim() !== ref.value) renameIn(ws, ref, to.trim());
  });
}

export function registerUsageMenu() {
  Blockly.ContextMenuRegistry.registry.register({
    id: 'puzzlebox_find_usages',
    scopeType: Blockly.ContextMenuRegistry.ScopeType.BLOCK,
    weight: 10,
    displayText: (scope) => {
      const ref = primaryRef(scope.block!)!;
      const n = usages(scope.block!.workspace as Blockly.WorkspaceSvg, ref).length;
      return `Find uses of ${ref.label} (${n})`;
    },
    preconditionFn: (scope) => (scope.block && !scope.block.isInFlyout && primaryRef(scope.block) ? 'enabled' : 'hidden'),
    callback: (scope) => showNext(scope.block!.workspace as Blockly.WorkspaceSvg, primaryRef(scope.block!)!),
  });
  Blockly.ContextMenuRegistry.registry.register({
    id: 'puzzlebox_rename_everywhere',
    scopeType: Blockly.ContextMenuRegistry.ScopeType.BLOCK,
    weight: 11,
    displayText: (scope) => `Rename ${primaryRef(scope.block!)!.label} everywhere…`,
    preconditionFn: (scope) => {
      const ref = scope.block && !scope.block.isInFlyout ? primaryRef(scope.block) : null;
      // Functions rename through their definition block already.
      return ref && ref.kind !== 'function' && ref.kind !== 'registry' && scope.block!.isEditable() ? 'enabled' : 'hidden';
    },
    callback: (scope) => renameEverywhere(scope.block!.workspace as Blockly.WorkspaceSvg, primaryRef(scope.block!)!),
  });
}
