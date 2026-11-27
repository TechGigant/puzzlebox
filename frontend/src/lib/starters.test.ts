// @vitest-environment jsdom
import * as Blockly from 'blockly/core';
import 'blockly/blocks';
import { beforeAll, describe, expect, it, vi } from 'vitest';
import { STARTERS } from './starters';
import { registerCoreBlocks } from './blocks/core';
import { registerConditionBlocks } from './blocks/conditions';
import { registerHelperBlocks } from './blocks/helpers';
import { registerDeviceBlocks } from './blocks/devices';
import { slotHint } from './editor/slot_hints';

beforeAll(() => {
  registerCoreBlocks();
  registerConditionBlocks();
  registerHelperBlocks();
  registerDeviceBlocks();
});

describe('starter programs', () => {
  it('load with every block and field they name', () => {
    const warn = vi.spyOn(console, 'warn');
    for (const s of STARTERS) {
      const ws = new Blockly.Workspace();
      Blockly.serialization.workspaces.load(JSON.parse(JSON.stringify(s.workspace)), ws);
      const when = ws.getTopBlocks(false);
      expect(when.map((b) => b.type), s.key).toEqual(['ha_when']);
      expect(when[0].getInputTargetBlock('TRIGGERS'), s.key).not.toBeNull();
      expect(when[0].getInputTargetBlock('DO'), s.key).not.toBeNull();
      // Loading drops what a block doesn't have; saving again shows it was all used.
      const saved = JSON.stringify(Blockly.serialization.workspaces.save(ws));
      for (const [, k, v] of JSON.stringify(s.workspace).matchAll(/"([A-Z_]+)":("[^"]*"|\d+)/g)) expect(saved, `${s.key} ${k}`).toContain(`"${k}":${v}`);
    }
    expect(warn).not.toHaveBeenCalled();
  });

  it('leave the entities to choose', () => {
    for (const s of STARTERS) {
      const ws = new Blockly.Workspace();
      Blockly.serialization.workspaces.load(JSON.parse(JSON.stringify(s.workspace)), ws);
      const entities = ws.getAllBlocks(false).flatMap((b) => [b.getField('ENTITY')].filter((f) => f));
      expect(entities.length, s.key).toBeGreaterThan(0);
      for (const f of entities) expect(f!.getValue(), s.key).toBe('');
    }
  });
});

describe('slot hints', () => {
  it('name what goes into the empty slots of a "when"', () => {
    const ws = new Blockly.Workspace();
    const when = Blockly.serialization.blocks.append({ type: 'ha_when' }, ws);
    expect(slotHint(when.getInput('TRIGGERS')!)).toContain('trigger');
    expect(slotHint(when.getInput('DO')!)).toContain('what should happen');
    Blockly.serialization.blocks.append({ type: 'ha_wait' }, ws).previousConnection!.connect(when.getInput('DO')!.connection!);
    expect(slotHint(when.getInput('DO')!)).toBeNull();
  });
});
