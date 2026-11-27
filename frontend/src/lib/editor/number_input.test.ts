// @vitest-environment jsdom
import * as Blockly from 'blockly/core';
import 'blockly/blocks';
import { beforeAll, describe, expect, it } from 'vitest';
import { decimalComma, installDecimalComma } from './number_input';
import { registerCoreBlocks } from '../blocks/core';

beforeAll(() => {
  installDecimalComma();
  registerCoreBlocks();
});

describe('decimal comma', () => {
  it('reads one comma as the decimal point', () => {
    expect(decimalComma('1,5')).toBe('1.5');
    expect(decimalComma('21,5')).toBe('21.5');
    expect(decimalComma('-0,25')).toBe('-0.25');
    expect(decimalComma('21.5')).toBe('21.5');
  });

  it('rejects text where a comma could separate thousands', () => {
    expect(decimalComma('1,234,5')).toBeNull();
    expect(decimalComma('1.234,5')).toBeNull();
    expect(decimalComma('1,234.5')).toBeNull();
  });

  it('applies to number fields', () => {
    const ws = new Blockly.Workspace();
    const n = Blockly.serialization.blocks.append({ type: 'math_number', fields: { NUM: 3 } }, ws);
    const field = n.getField('NUM')!;
    field.setValue('21,5');
    expect(field.getValue()).toBe(21.5);
    field.setValue('1,234,5');
    expect(field.getValue()).toBe(21.5);
    // Blockly's own clean-ups still apply.
    field.setValue('1O');
    expect(field.getValue()).toBe(10);
  });

  it('applies to the number fields of Puzzlebox blocks', () => {
    const ws = new Blockly.Workspace();
    // Without events: blocks like function definitions react to them later, after the test.
    Blockly.Events.disable();
    const numberFields = Object.keys(Blockly.Blocks).flatMap((type) => {
      try {
        const b = ws.newBlock(type);
        return b.inputList.flatMap((i) => i.fieldRow).filter((f) => f instanceof Blockly.FieldNumber);
      } catch {
        return [];
      }
    });
    expect(numberFields.length).toBeGreaterThan(1);
    for (const f of numberFields) {
      f.setValue('7.5');
      const withPoint = f.getValue();
      f.setValue('3');
      f.setValue('7,5');
      expect(f.getValue(), f.getSourceBlock()?.type).toBe(withPoint);
    }
    ws.dispose();
    Blockly.Events.enable();
  });
});
