// @vitest-environment jsdom
import * as Blockly from 'blockly/core';
import 'blockly/blocks';
import * as En from 'blockly/msg/en';
import { beforeAll, describe, expect, it } from 'vitest';
import { deleteText, installMenuText } from './menus';
import { registerCoreBlocks } from '../blocks/core';

beforeAll(() => {
  Blockly.setLocale(En as unknown as Record<string, string>);
  registerCoreBlocks();
  installMenuText();
});

const log = { type: 'ha_log', inputs: { MESSAGE: { shadow: { type: 'text', fields: { TEXT: 'hi' } } } } };

describe('menu text', () => {
  it('is in sentence case', () => {
    expect(Blockly.Msg['ADD_COMMENT']).toBe('Add comment');
    expect(Blockly.Msg['COLLAPSE_BLOCK']).toBe('Collapse block');
    expect(Blockly.Msg['DELETE_X_BLOCKS']).toBe('Delete %1 blocks');
  });

  it('says which blocks a delete takes along', () => {
    const ws = new Blockly.Workspace();
    const make = (state: Blockly.serialization.blocks.State) => Blockly.serialization.blocks.append(state, ws);
    expect(deleteText(make(log))).toBe('Delete block');
    const ifBlock = make({ type: 'controls_if', inputs: { DO0: { block: { ...log, next: { block: log } } } }, next: { block: log } });
    // The blocks under it stay; the shadows in its sockets don't count.
    expect(deleteText(ifBlock)).toBe('Delete this and the 2 blocks inside');
    expect(deleteText(make({ type: 'controls_if', inputs: { DO0: { block: log } } }))).toBe('Delete this and the block inside');
  });

  it('replaces the delete text of the block menu', () => {
    const item = Blockly.ContextMenuRegistry.registry.getItem('blockDelete') as unknown as { displayText: (s: object) => string };
    const ws = new Blockly.Workspace();
    expect(item.displayText({ block: Blockly.serialization.blocks.append(log, ws) })).toBe('Delete block');
  });
});
