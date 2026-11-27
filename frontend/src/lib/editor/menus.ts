//! Blockly's menu texts in the sentence case the rest of Puzzlebox uses.

import * as Blockly from 'blockly/core';

const MESSAGES: Record<string, string> = {
  ADD_COMMENT: 'Add comment',
  REMOVE_COMMENT: 'Remove comment',
  DUPLICATE_COMMENT: 'Duplicate comment',
  COLLAPSE_BLOCK: 'Collapse block',
  EXPAND_BLOCK: 'Expand block',
  COLLAPSE_ALL: 'Collapse blocks',
  EXPAND_ALL: 'Expand blocks',
  DISABLE_BLOCK: 'Disable block',
  ENABLE_BLOCK: 'Enable block',
  DELETE_BLOCK: 'Delete block',
  DELETE_X_BLOCKS: 'Delete %1 blocks',
  CLEAN_UP: 'Clean up blocks',
  INLINE_INPUTS: 'Inline inputs',
  EXTERNAL_INPUTS: 'External inputs',
  EDIT_BLOCK_CONTENTS: 'Edit block contents',
  MOVE_BLOCK: 'Move block',
  ARIA_LABEL_COMMENT_COLLAPSE: 'Collapse comment',
  ARIA_LABEL_COMMENT_EXPAND: 'Expand comment',
  ICON_LABEL_COMMENT_CLOSED: 'Open comment',
  ICON_LABEL_COMMENT_OPEN: 'Close comment',
  ICON_LABEL_WARNING_CLOSED: 'Open warning',
  ICON_LABEL_WARNING_OPEN: 'Close warning',
  COPY_TO_BACKPACK: 'Copy to backpack',
  COPY_ALL_TO_BACKPACK: 'Copy all blocks to backpack',
  PASTE_ALL_FROM_BACKPACK: 'Paste all blocks from backpack',
  REMOVE_FROM_BACKPACK: 'Remove from backpack',
  EMPTY_BACKPACK: 'Empty backpack',
  // Shown in menus next to the item ("Ctrl + Z").
  CONTROL_KEY: 'Ctrl',
};

/** "Delete 3 blocks" on what looks like one block: say where the others are. */
export function deleteText(block: Blockly.Block): string {
  const count = (b: Blockly.Block | null) => (b ? b.getDescendants(false).filter((d) => !d.isShadow()).length : 0);
  const inside = count(block) - count(block.getNextBlock()) - 1;
  if (inside <= 0) return 'Delete block';
  return inside === 1 ? 'Delete this and the block inside' : `Delete this and the ${inside} blocks inside`;
}

/** Blocks "delete all" removes, without the shadows (the default values in sockets). */
const deletable = (ws: Blockly.WorkspaceSvg) => ws.getAllBlocks(false).filter((b) => b.isDeletable() && !b.isShadow()).length;

let installed = false;
/** Call after `Blockly.setLocale`, which would put the English texts back. */
export function installMenuText() {
  if (installed) return;
  installed = true;
  Object.assign(Blockly.Msg, MESSAGES);
  const registry = Blockly.ContextMenuRegistry.registry;
  const del = registry.getItem('blockDelete');
  if (del && 'displayText' in del) del.displayText = (scope) => (scope.block ? deleteText(scope.block) : 'Delete block');
  const all = registry.getItem('workspaceDelete');
  if (all && 'displayText' in all) {
    all.displayText = (scope) => {
      const n = scope.workspace ? deletable(scope.workspace) : 0;
      return n === 1 ? 'Delete block' : `Delete all ${n} blocks`;
    };
    const callback = all.callback;
    all.callback = (scope, ...rest) => {
      // Blockly's "Delete all 8 blocks?" counts the shadows too.
      const original = Blockly.Msg['DELETE_ALL_BLOCKS'];
      if (scope.workspace) Blockly.Msg['DELETE_ALL_BLOCKS'] = `Delete all ${deletable(scope.workspace)} blocks?`;
      try {
        callback?.(scope, ...rest);
      } finally {
        Blockly.Msg['DELETE_ALL_BLOCKS'] = original;
      }
    };
  }
  // Their shortcuts are bare letters ("D", "C"), which read like stray text; the shortcuts help lists them.
  for (const id of ['blockDuplicate', 'cleanWorkspace', 'commentDuplicate']) {
    const item = registry.getItem(id) as { associatedKeyboardShortcut?: string } | null;
    if (item) delete item.associatedKeyboardShortcut;
  }
}
