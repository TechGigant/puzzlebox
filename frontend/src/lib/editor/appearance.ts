//! Editor look: compact blocks and alternating shades for nested blocks of one colour.

import * as Blockly from 'blockly/core';

export interface EditorPrefs {
  /** Nested blocks of the same colour alternate between two shades. */
  zebra: boolean;
  /** Smaller blocks (less padding), for big programs. */
  compact: boolean;
}

const KEY = 'puzzlebox.editor';
const DEFAULTS: EditorPrefs = { zebra: true, compact: false };

export function loadPrefs(): EditorPrefs {
  try {
    return { ...DEFAULTS, ...JSON.parse(localStorage.getItem(KEY) ?? '{}') };
  } catch {
    return { ...DEFAULTS };
  }
}

export function savePrefs(p: EditorPrefs) {
  try {
    localStorage.setItem(KEY, JSON.stringify(p));
  } catch {
    /* storage unavailable: the choice lasts for this page only */
  }
}

class CompactRenderer extends Blockly.zelos.Renderer {
  protected override makeConstants_() {
    return new Blockly.zelos.ConstantProvider(3);
  }
}

let registered = false;
export function rendererName(p: EditorPrefs): string {
  if (!registered) {
    registered = true;
    Blockly.blockRendering.register('pb_compact', CompactRenderer);
  }
  return p.compact ? 'pb_compact' : 'zelos';
}

export const ALT = '_alt';

/**
 * A block nested directly inside a block of the same style takes the other shade, so deep
 * if-in-if or loop-in-loop code stays readable.
 */
export function applyZebra(ws: Blockly.WorkspaceSvg, on: boolean) {
  const base = (b: Blockly.Block) => b.getStyleName().replace(/_alt$/, '');
  const striped = new Map<string, boolean>();
  const isStriped = (b: Blockly.Block): boolean => {
    const known = striped.get(b.id);
    if (known !== undefined) return known;
    const parent = b.getSurroundParent();
    const value = on && !!parent && !b.isShadow() && base(parent) === base(b) && !isStriped(parent);
    striped.set(b.id, value);
    return value;
  };
  for (const b of ws.getAllBlocks(false)) {
    if (b.isShadow()) continue;
    const want = base(b) + (isStriped(b) ? ALT : '');
    if (b.getStyleName() !== want && ws.getTheme().blockStyles[want]) b.setStyle(want);
  }
}

const ENTRY = /^(ha_when|ha_on_run|ha_on_start|procedures_def)/;

/** Stacks outside any entry block never run on their own; they're drawn faded. */
export function markLoose(ws: Blockly.WorkspaceSvg) {
  for (const b of ws.getTopBlocks(false) as Blockly.BlockSvg[]) {
    b.getSvgRoot().classList.toggle('pb-loose', !ENTRY.test(b.type));
  }
}
