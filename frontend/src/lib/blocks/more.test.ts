import { describe, expect, it } from 'vitest';
import * as Blockly from 'blockly/core';
import 'blockly/blocks';
import * as En from 'blockly/msg/en';
import { registerCoreBlocks } from './core';
import { registerHelperBlocks } from './helpers';
import { MORE_BLOCKS, setProgramsForTest } from './more';
import { buildToolbox } from '../toolbox';
import { search, toolboxEntries } from '../editor/quickadd';

Blockly.setLocale(En as unknown as Record<string, string>);
registerCoreBlocks();
registerHelperBlocks();

const load = (state: Blockly.serialization.blocks.State, ws = new Blockly.Workspace()) =>
  Blockly.serialization.blocks.append(state, ws, { recordUndo: false });

/** Category names in the toolbox that list a block. */
function categoriesOf(type: string): string[] {
  const found: string[] = [];
  const walk = (items: unknown[], cat: string) => {
    for (const i of items as { kind: string; name?: string; type?: string; contents?: unknown[] }[]) {
      if (i.kind === 'category') walk(i.contents ?? [], i.name!);
      else if (i.type === type) found.push(cat);
    }
  };
  walk(buildToolbox().contents as unknown[], '');
  return found;
}

describe('list, program and fade blocks', () => {
  it('are in the toolbox where people look for them', () => {
    const entries = toolboxEntries(buildToolbox());
    expect(MORE_BLOCKS.filter((d) => !entries.some((e) => e.state.type === d.type)).map((d) => d.type)).toEqual([]);
    expect(categoriesOf('lists_add')).toEqual(['Lists']);
    expect(categoriesOf('program_run_wait')).toEqual(['Control']);
    expect(categoriesOf('light_fade')).toEqual(['Lights & colours']);
    for (const t of ['control_try', 'control_carry_on', 'control_parallel']) expect(categoriesOf(t)).toEqual(['Control']);
    expect(search(entries, 'fade')[0].state.type).toBe('light_fade');
    expect(search(entries, 'run program')[0].state.type).toMatch(/^program_run/);
  });

  it('read like sentences and show the program by name', () => {
    setProgramsForTest([{ id: 'abc123', name: 'Wake up', enabled: true }]);
    expect(load({ type: 'program_run_wait', fields: { PROGRAM: 'abc123' } }).toString()).toBe(
      'run program Wake up and wait until it finishes',
    );
    expect(load({ type: 'program_run', fields: { PROGRAM: 'gone' } }).toString()).toBe('run program a deleted program');
    expect(load({ type: 'lists_contains' }).toString()).toBe('list ? contains ?');
    expect(load({ type: 'light_fade' }).toString()).toBe('fade ? from ? % to ? % over ?');
  });

  it('turn a typed program name into its id', () => {
    setProgramsForTest([{ id: 'abc123', name: 'Wake up', enabled: true }]);
    const b = load({ type: 'program_run' });
    b.setFieldValue('wake up', 'PROGRAM');
    expect(b.getFieldValue('PROGRAM')).toBe('abc123');
  });
});
