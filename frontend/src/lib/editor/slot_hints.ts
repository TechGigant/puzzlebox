//! "drop a ⚡ trigger here" next to the empty slots of "when" blocks. Drawn on top of the
//! blocks, not as fields, so they aren't saved, compiled or counted.

import * as Blockly from 'blockly/core';

const ENTRY = new Set(['ha_when', 'ha_on_run', 'ha_on_start']);
const SVG_NS = 'http://www.w3.org/2000/svg';

export function slotHint(input: Blockly.Input): string | null {
  if (input.type !== Blockly.inputs.inputTypes.STATEMENT || input.connection?.targetBlock()) return null;
  return input.connection?.getCheck()?.includes('Trigger') ? '← drop a ⚡ trigger here' : '← drop what should happen here';
}

function draw(ws: Blockly.WorkspaceSvg) {
  for (const old of ws.getCanvas().querySelectorAll('.pb-slot-hint')) old.remove();
  for (const b of ws.getTopBlocks(false)) {
    if (!ENTRY.has(b.type) || b.isCollapsed() || !b.isEnabled() || b.isDeadOrDying()) continue;
    const width = b.getHeightWidth().width;
    for (const input of b.inputList) {
      const text = slotHint(input);
      if (!text) continue;
      const offset = (input.connection as Blockly.RenderedConnection).getOffsetInBlock();
      const el = document.createElementNS(SVG_NS, 'text');
      el.setAttribute('class', 'pb-slot-hint');
      el.setAttribute('aria-hidden', 'true');
      el.setAttribute('x', String(width + 12));
      el.setAttribute('y', String(offset.y + 16));
      el.textContent = text;
      b.getSvgRoot().appendChild(el);
    }
  }
}

export function installSlotHints(ws: Blockly.WorkspaceSvg): () => void {
  let queued = false;
  const listener = (e: Blockly.Events.Abstract) => {
    if (queued || (e.isUiEvent && e.type !== Blockly.Events.THEME_CHANGE)) return;
    queued = true;
    // After Blockly has rendered the change, so the slots are where they will be shown.
    requestAnimationFrame(() =>
      Blockly.renderManagement.finishQueuedRenders().then(() => {
        queued = false;
        if (ws.getParentSvg().isConnected) draw(ws);
      }),
    );
  };
  ws.addChangeListener(listener);
  return () => ws.removeChangeListener(listener);
}
