//! "Run as if this fired" on trigger blocks: runs the program's "when" with made-up trigger
//! data, so programs that read the trigger can be tested without waiting for it.

import * as Blockly from 'blockly/core';

let handler: ((triggerBlockId: string) => void) | null = null;
let registered = false;

/** Sets what the menu item does for the open editor; returns a function that unsets it. */
export function setRunAsTriggerHandler(h: (triggerBlockId: string) => void): () => void {
  register();
  handler = h;
  return () => {
    if (handler === h) handler = null;
  };
}

function register() {
  if (registered) return;
  registered = true;
  Blockly.ContextMenuRegistry.registry.register({
    id: 'puzzlebox_run_as_trigger',
    scopeType: Blockly.ContextMenuRegistry.ScopeType.BLOCK,
    weight: -2,
    displayText: () => 'Run as if ⚡ this fired',
    preconditionFn: (scope) => {
      const b = scope.block;
      if (!b || b.isInFlyout || !handler) return 'hidden';
      if (!b.previousConnection?.getCheck()?.includes('Trigger')) return 'hidden';
      return b.getRootBlock().type === 'ha_when' ? 'enabled' : 'hidden';
    },
    callback: (scope) => handler?.(scope.block!.id),
  });
}
