import { describe, expect, it } from 'vitest';
import * as Blockly from 'blockly/core';
import { registerCoreBlocks } from './core';

registerCoreBlocks();

const load = (state: Blockly.serialization.blocks.State) => {
  const ws = new Blockly.Workspace();
  return Blockly.serialization.blocks.append(state, ws, { recordUndo: false });
};
const settle = () => new Promise((r) => setTimeout(r));

describe('trigger blocks', () => {
  it('loads old saved shapes with defaults that compile like before', () => {
    const state = load({ type: 'ha_trigger_state', fields: { ENTITY: 'light.a', FROM: 'off', TO: 'on' } });
    expect(state.getFieldValue('WHAT')).toBe('state');
    expect(state.getFieldValue('TO_OP')).toBe('is');
    expect(load({ type: 'ha_trigger_ha_start' }).getFieldValue('EVENT')).toBe('start');
    const time = load({ type: 'ha_trigger_time', fields: { AT: '07:00' } });
    expect(['MON', 'TUE', 'WED', 'THU', 'FRI', 'SAT', 'SUN'].map((d) => time.getFieldValue(d))).toEqual(Array(7).fill('TRUE'));
  });

  it('shows options only where they apply', async () => {
    const state = load({ type: 'ha_trigger_state', fields: { ENTITY: 'light.a', WHAT: 'attribute', ATTRIBUTE: 'brightness' } });
    await settle();
    expect(state.getField('ATTRIBUTE')!.isVisible()).toBe(true);
    state.setFieldValue('any', 'WHAT');
    await settle();
    expect(state.getField('ATTRIBUTE')!.isVisible()).toBe(false);
    expect(state.getField('TO')!.isVisible()).toBe(false);

    const pattern = load({ type: 'ha_trigger_time_pattern' });
    expect(pattern.getField('HOURS')!.isVisible()).toBe(false);
    expect(pattern.getField('MINUTES')!.isVisible()).toBe(true);
  });
});
