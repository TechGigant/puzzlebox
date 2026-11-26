import { describe, expect, it } from 'vitest';
import * as Blockly from 'blockly/core';
import { registerCoreBlocks } from './core';

registerCoreBlocks();

const load = (state: Blockly.serialization.blocks.State) => {
  const ws = new Blockly.Workspace();
  return Blockly.serialization.blocks.append(state, ws, { recordUndo: false });
};
const settle = () => new Promise((r) => setTimeout(r));

describe('wait blocks', () => {
  it('hide the time while there is no time limit', async () => {
    for (const type of ['ha_wait_for_trigger', 'wait_until']) {
      const wait = load({ type, fields: { LIMIT: 'none' } });
      await settle();
      expect(wait.getInput('TIMEOUT')!.isVisible()).toBe(false);
      wait.setFieldValue('limit', 'LIMIT');
      await settle();
      expect(wait.getInput('TIMEOUT')!.isVisible()).toBe(true);
      expect(load({ type }).getFieldValue('LIMIT')).toBe('limit');
    }
  });

  it('carry on if this fails holds a stack', () => {
    const b = load({ type: 'control_carry_on', inputs: { DO: { block: { type: 'ha_wait' } } } });
    expect(b.getInputTargetBlock('DO')?.type).toBe('ha_wait');
  });
});
