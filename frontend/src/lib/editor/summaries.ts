//! Readable one-line text on collapsed blocks: "when motion turns on (+1) → 6 blocks"
//! instead of Blockly's first 30 characters.

import * as Blockly from 'blockly/core';

const COLLAPSED_FIELD = '_TEMP_COLLAPSED_FIELD';

const plain = (b: Blockly.Block, max: number) => b.toString(max).replace(/[＋+] option/g, '').replace(/\s+/g, ' ').trim();

function countInside(b: Blockly.Block): number {
  return b.getDescendants(false).filter((d) => d !== b && !d.isShadow()).length;
}

function chain(first: Blockly.Block | null): Blockly.Block[] {
  const out: Blockly.Block[] = [];
  for (let b = first; b; b = b.getNextBlock()) out.push(b);
  return out;
}

export function summary(b: Blockly.Block): string {
  const n = countInside(b);
  const blocks = n === 1 ? '1 block' : `${n} blocks`;
  if (b.type === 'ha_when') {
    const triggers = chain(b.getInputTargetBlock('TRIGGERS'));
    const first = triggers[0] ? plain(triggers[0], 40) : 'no trigger';
    const more = triggers.length > 1 ? ` (+${triggers.length - 1})` : '';
    return `when ${first}${more} → ${blocks}`;
  }
  if (b.type === 'ha_on_run') return `when run manually → ${blocks}`;
  if (b.type === 'ha_on_start') return `when program starts → ${blocks}`;
  if (b.type.startsWith('procedures_def')) return `function ${b.getFieldValue('NAME')} → ${blocks}`;
  const own = plain(b, 40);
  return n ? `${own} … (${blocks})` : own;
}

function update(b: Blockly.BlockSvg) {
  if (!b.isCollapsed()) return;
  const field = b.getField(COLLAPSED_FIELD);
  const text = summary(b);
  if (field && field.getValue() !== text) field.setValue(text);
}

let patched = false;
/** Blockly builds a collapsed block's text with `toString(COLLAPSE_CHARS)`; answer with ours. */
function patchCollapsedText() {
  if (patched) return;
  patched = true;
  const proto = Blockly.BlockSvg.prototype;
  const original = proto.toString;
  proto.toString = function (this: Blockly.BlockSvg, maxLength?: number, emptyToken?: string) {
    if (maxLength === Blockly.COLLAPSE_CHARS && this.isCollapsed()) {
      // `summary` calls toString with other lengths, so this doesn't recurse.
      return summary(this);
    }
    return original.call(this, maxLength, emptyToken);
  };
}

/** Keeps collapsed blocks' text up to date; returns a cleanup function. */
export function installSummaries(ws: Blockly.WorkspaceSvg): () => void {
  patchCollapsedText();
  let queued = false;
  const refresh = () => {
    if (queued) return;
    queued = true;
    requestAnimationFrame(() => {
      queued = false;
      for (const b of ws.getTopBlocks(false)) {
        for (const d of b.getDescendants(false)) update(d as Blockly.BlockSvg);
      }
    });
  };
  const listener = (e: Blockly.Events.Abstract) => {
    if (!e.isUiEvent || e.type === Blockly.Events.FINISHED_LOADING) refresh();
  };
  ws.addChangeListener(listener);
  refresh();
  return () => ws.removeChangeListener(listener);
}
