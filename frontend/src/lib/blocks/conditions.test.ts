// @vitest-environment jsdom
import * as Blockly from 'blockly/core';
import { beforeAll, describe, expect, it } from 'vitest';
import { registerCoreBlocks } from './core';
import { registerConditionBlocks } from './conditions';

beforeAll(() => {
  registerCoreBlocks();
  registerConditionBlocks();
});

const load = (state: Blockly.serialization.blocks.State) => Blockly.serialization.blocks.append(state, new Blockly.Workspace());
const save = (b: Blockly.Block) => Blockly.serialization.blocks.save(b, { addCoordinates: false, addInputBlocks: false });

describe('condition settings', () => {
  it('saves and loads settings with their fields', () => {
    const b = load({ type: 'ha_is_state', extraState: { options: ['ATTRIBUTE', 'MATCH'] }, fields: { STATE: '255', ATTRIBUTE: 'brightness', MATCH: 'any' } });
    const s = save(b)!;
    expect(s.extraState).toEqual({ options: ['ATTRIBUTE', 'MATCH'] });
    expect(s.fields).toMatchObject({ STATE: '255', ATTRIBUTE: 'brightness', MATCH: 'any' });
  });

  it('keeps old blocks as they were', () => {
    const s = save(load({ type: 'ha_is_state', fields: { STATE: 'on' } }))!;
    expect(s.extraState).toBeUndefined();
    expect(Object.keys(s.fields ?? {})).toEqual(['STATE']);
  });

  it('removes settings on undo-style reloads', () => {
    const b = load({ type: 'sun_condition', extraState: { options: ['SECOND'] }, fields: { WHEN2: 'before', EVENT2: 'sunrise' } });
    expect(b.getField('WHEN2')).not.toBeNull();
    b.loadExtraState!({ options: [] });
    expect(b.getField('WHEN2')).toBeNull();
    expect(b.getInput('SECOND')).toBeNull();
    expect(save(b)!.extraState).toBeUndefined();
  });
});
