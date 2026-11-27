import { describe, expect, it } from 'vitest';
import * as Blockly from 'blockly/core';
import 'blockly/blocks';
import * as En from 'blockly/msg/en';
import { registerCoreBlocks } from './core';
import { registerHelperBlocks } from './helpers';
import { EVERYDAY_BLOCKS, rememberedNames } from './everyday';
import { buildToolbox } from '../toolbox';
import { search, toolboxEntries } from '../editor/quickadd';
import { alternatives } from '../editor/switching';

Blockly.setLocale(En as unknown as Record<string, string>);
registerCoreBlocks();
registerHelperBlocks();

const load = (state: Blockly.serialization.blocks.State, ws = new Blockly.Workspace()) =>
  Blockly.serialization.blocks.append(state, ws, { recordUndo: false });
const settle = () => new Promise((r) => setTimeout(r));

describe('everyday blocks', () => {
  it('are all in the toolbox and found by quick add', () => {
    const entries = toolboxEntries(buildToolbox());
    expect(EVERYDAY_BLOCKS.filter((def) => !entries.some((e) => e.state.type === def.type)).map((d) => d.type)).toEqual([]);
    expect(search(entries, 'first person arrives')[0].state.type).toBe('ha_trigger_occupancy');
    expect(search(entries, 'press')[0].state.type).toBe('button_press');
    expect(search(entries, 'disarm')[0].state.type).toBe('alarm_disarm');
  });

  it('read like sentences', () => {
    expect(load({ type: 'ha_trigger_occupancy' }).toString()).toContain('the first person arrives at');
    expect(load({ type: 'scene_remember', fields: { NAME: 'before' } }).toString()).toBe('remember the state of ? as before');
    expect(load({ type: 'alarm_arm' }).toString()).toBe('arm ? as away with code ?');
  });

  it('ask hides the time while there is no time limit', async () => {
    const ask = load({ type: 'notify_ask', fields: { LIMIT: 'none' } });
    await settle();
    expect(ask.getInput('TIMEOUT')!.isVisible()).toBe(false);
  });

  it('restore suggests the names remembered in the program', () => {
    const ws = new Blockly.Workspace();
    load({ type: 'scene_remember', fields: { NAME: 'movie' } }, ws);
    load({ type: 'scene_remember', fields: { NAME: 'before' } }, ws);
    load({ type: 'scene_remember', fields: { NAME: 'movie' } }, ws);
    expect(rememberedNames(ws)).toEqual(['before', 'movie']);
  });

  it('can be switched with their neighbours', () => {
    const labels = (type: string) => alternatives(load({ type })).map((a) => a.type);
    expect(labels('notify_send')).toContain('notify_ask');
    expect(labels('ha_trigger_zone')).toContain('ha_trigger_occupancy');
    expect(labels('alarm_arm')).toEqual(['alarm_disarm']);
  });

  it('variable blocks say how long the variable lasts', () => {
    const ws = new Blockly.Workspace();
    const tip = (type: string) => {
      const v = ws.getVariableMap().createVariable(`v_${type}`, type);
      const b = load({ type: 'variables_get', fields: { VAR: { id: v.getId() } } }, ws);
      return Blockly.Tooltip.getTooltipOfObject(b);
    };
    expect(tip('')).toContain('only during this run');
    expect(tip('Program')).toContain('kept between runs of this program');
    expect(tip('Global')).toContain('shared by all programs');
  });
});
