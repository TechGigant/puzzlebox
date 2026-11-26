// @vitest-environment jsdom
import * as Blockly from 'blockly/core';
import { beforeAll, describe, expect, it } from 'vitest';
import { registerCoreBlocks } from './core';

beforeAll(() => registerCoreBlocks());

const load = (state: Blockly.serialization.blocks.State) => Blockly.serialization.blocks.append(state, new Blockly.Workspace());

describe('"like Home Assistant" value blocks', () => {
  it.each([
    ['to_number_ha', { KIND: 'int' }, ['VALUE', 'DEFAULT']],
    ['math_round_ha', { DECIMALS: 2, METHOD: 'half' }, ['NUM']],
    ['to_text_ha', { NUMBERS: 'float' }, ['VALUE']],
    ['logic_compare_strict', { OP: 'LT' }, ['A', 'B']],
    ['logic_is_null', {}, ['VALUE']],
    ['time_format', { FORMAT: '%-d.%-m.' }, ['DATETIME']],
  ])('%s keeps its fields and inputs', (type, fields, inputs) => {
    const b = load({ type, fields });
    for (const [k, v] of Object.entries(fields)) expect(b.getFieldValue(k)).toBe(v);
    for (const i of inputs) expect(b.getInput(i)).not.toBeNull();
    expect(b.outputConnection).not.toBeNull();
  });
});
