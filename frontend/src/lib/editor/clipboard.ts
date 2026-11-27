//! Copy in one program, paste in another (or in another tab). Blockly pastes into the workspace
//! the blocks were copied from, which is gone once that program is closed.

import * as Blockly from 'blockly/core';
import { freeSpot } from './quickadd';

const KEY = 'puzzlebox.clipboard';

type Shortcut = Blockly.ShortcutRegistry.KeyboardShortcut;
/** What was copied, and the id of the workspace it was copied from. */
export interface Copied {
  data: Blockly.ICopyData;
  from: string;
}

let last: Copied | null = null;

/** What was copied last, in any program or tab. */
export function storedCopy(): Copied | null {
  try {
    const raw = localStorage.getItem(KEY);
    if (raw) return JSON.parse(raw) as Copied;
  } catch {
    /* fall back to this tab's copy */
  }
  return last;
}

export function storeCopy(data: Blockly.ICopyData | null, ws: Blockly.WorkspaceSvg) {
  if (!data) return;
  last = { data, from: (ws.isFlyout ? ws.targetWorkspace : ws)?.id ?? ws.id };
  try {
    localStorage.setItem(KEY, JSON.stringify(last));
  } catch {
    /* pasting still works in this tab */
  }
}

/** Copied blocks keep where they were in their program; in another one they go in view. */
export function placeInView(data: Blockly.ICopyData, ws: Blockly.WorkspaceSvg): Blockly.ICopyData {
  const state = (data as { blockState?: Blockly.serialization.blocks.State }).blockState;
  if (!state) return data;
  const view = ws.getMetricsManager().getViewMetrics(true);
  const at = { x: Math.round(view.left + view.width / 3), y: Math.round(view.top + view.height / 3) };
  return { ...data, blockState: { ...state, ...at } } as Blockly.ICopyData;
}

/** Points Blockly's clipboard at `ws`, with the newest copy from any tab; true if it came from elsewhere. */
export function preparePaste(ws: Blockly.WorkspaceSvg): boolean {
  const main = ws.isFlyout ? ws.targetWorkspace : ws;
  const copied = storedCopy();
  if (!main || !copied) return false;
  const elsewhere = copied.from !== main.id;
  const data = elsewhere ? placeInView(copied.data, main) : copied.data;
  Blockly.clipboard.setLastCopiedData(data);
  Blockly.clipboard.setLastCopiedWorkspace(main);
  if (elsewhere) {
    const s = (data as { blockState?: { x?: number; y?: number } }).blockState;
    if (s?.x !== undefined && s.y !== undefined) Blockly.clipboard.setLastCopiedLocation(new Blockly.utils.Coordinate(s.x, s.y));
  }
  return elsewhere;
}

/** Blockly only nudges a paste off a block at exactly the same spot; move it clear of all of them. */
function clearOfOthers(ws: Blockly.WorkspaceSvg, before: Set<string>) {
  // Sizes are known once the pasted blocks have been drawn.
  Blockly.renderManagement.finishQueuedRenders().then(() => {
    for (const b of ws.getTopBlocks(false).filter((t) => !before.has(t.id))) {
      const others = ws.getTopBlocks(false).filter((o) => o !== b).map((o) => o.getBoundingRectangle());
      const { width, height } = b.getHeightWidth();
      b.moveTo(freeSpot(others, width, height, b.getRelativeToSurfaceXY()));
    }
  });
}

function wrap(name: string, change: (s: Shortcut) => Partial<Shortcut>) {
  const registry = Blockly.ShortcutRegistry.registry;
  const s = registry.getRegistry()[name];
  if (!s) return;
  registry.unregister(name);
  registry.register({ ...s, ...change(s) });
}

let installed = false;
export function installSharedClipboard() {
  if (installed) return;
  installed = true;
  for (const name of [Blockly.ShortcutItems.names.COPY, Blockly.ShortcutItems.names.CUT]) {
    wrap(name, (s) => ({
      callback: (ws, e, shortcut, scope) => {
        const done = s.callback?.(ws, e, shortcut, scope) ?? false;
        if (done) storeCopy(Blockly.clipboard.getLastCopiedData(), ws);
        return done;
      },
    }));
  }
  let elsewhere = false;
  wrap(Blockly.ShortcutItems.names.PASTE, (s) => ({
    preconditionFn: (ws, scope) => {
      elsewhere = preparePaste(ws);
      return s.preconditionFn?.(ws, scope) ?? true;
    },
    callback: (ws, e, shortcut, scope) => {
      const main = (ws.isFlyout ? ws.targetWorkspace : ws) ?? ws;
      const before = new Set(main.getTopBlocks(false).map((b) => b.id));
      const done = s.callback?.(ws, e, shortcut, scope) ?? false;
      if (done && elsewhere) clearOfOthers(main, before);
      return done;
    },
  }));
}
