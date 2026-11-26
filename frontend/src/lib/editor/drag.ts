//! Drag and paste helpers: Alt+drag leaves a copy behind, Ctrl+V pastes at the pointer.
//! (Ctrl/Cmd+drag pulling out a single block is built into Blockly.)

import * as Blockly from 'blockly/core';

interface Pending {
  id: string;
  state: Blockly.serialization.blocks.State;
  parent: Blockly.Connection | null;
  xy: Blockly.utils.Coordinate;
}

/**
 * Wires the helpers to a workspace; returns a cleanup function. `pointer` gives the last
 * pointer position in screen coordinates (null when it hasn't moved over the canvas).
 */
export function installDragTricks(
  ws: Blockly.WorkspaceSvg,
  host: HTMLElement,
  pointer: () => { clientX: number; clientY: number } | null,
): () => void {
  let pending: Pending | null = null;

  const onPointerDown = (e: PointerEvent) => {
    pending = null;
    if (!e.altKey || e.button !== 0) return;
    const el = (e.target as Element | null)?.closest('[data-id]');
    const block = el ? (ws.getBlockById(el.getAttribute('data-id') ?? '') as Blockly.BlockSvg | null) : null;
    if (!block || block.isShadow() || !block.isMovable() || !block.isDeletable()) return;
    pending = {
      id: block.id,
      state: Blockly.serialization.blocks.save(block, { addNextBlocks: true, saveIds: false })!,
      parent: block.outputConnection?.targetConnection ?? block.previousConnection?.targetConnection ?? null,
      xy: block.getRelativeToSurfaceXY(),
    };
  };

  const onDrag = (e: Blockly.Events.Abstract) => {
    if (e.type !== Blockly.Events.BLOCK_DRAG || !pending) return;
    const drag = e as Blockly.Events.BlockDrag;
    if (!drag.isStart || drag.blockId !== pending.id) return;
    const { state, parent, xy } = pending;
    pending = null;
    // The original is being dragged away; put the copy where it was.
    const copy = Blockly.serialization.blocks.append(state, ws) as Blockly.BlockSvg;
    const own = copy.outputConnection ?? copy.previousConnection;
    if (parent && own && !parent.targetBlock() && ws.connectionChecker.canConnect(parent, own, false)) parent.connect(own);
    else copy.moveTo(xy);
  };

  const onKey = (e: KeyboardEvent) => {
    if (!(e.ctrlKey || e.metaKey) || e.key.toLowerCase() !== 'v' || e.shiftKey || e.altKey) return;
    if ((e.target as HTMLElement)?.closest('input, textarea, [contenteditable]')) return;
    const data = Blockly.clipboard.getLastCopiedData();
    const p = pointer();
    if (!data || !p || Blockly.clipboard.getLastCopiedWorkspace() !== ws) return;
    // Our paste replaces Blockly's, which drops the copy next to the original.
    e.preventDefault();
    e.stopPropagation();
    const at = Blockly.utils.svgMath.screenToWsCoordinates(ws, new Blockly.utils.Coordinate(p.clientX, p.clientY));
    Blockly.clipboard.paste(data, ws, at);
  };

  host.addEventListener('pointerdown', onPointerDown, true);
  host.addEventListener('keydown', onKey, true);
  ws.addChangeListener(onDrag);
  return () => {
    host.removeEventListener('pointerdown', onPointerDown, true);
    host.removeEventListener('keydown', onKey, true);
    ws.removeChangeListener(onDrag);
  };
}
