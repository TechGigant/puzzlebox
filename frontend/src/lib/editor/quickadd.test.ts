// @vitest-environment jsdom
import * as Blockly from 'blockly/core';
import 'blockly/blocks';
import { beforeAll, describe, expect, it } from 'vitest';
import { attach, intoEmptyWhen, search, toolboxEntries, wantFor, type Entry } from './quickadd';
import { registerCoreBlocks } from '../blocks/core';
import { registerConditionBlocks } from '../blocks/conditions';
import { registerHelperBlocks } from '../blocks/helpers';
import { registerDeviceBlocks } from '../blocks/devices';
import { buildToolbox } from '../toolbox';

const entry = (label: string, path = 'Home Assistant › Light', more: Partial<Entry> = {}): Entry => ({
  label,
  path,
  state: { type: 'x' },
  haystack: `${label} ${path}`.toLowerCase(),
  ...more,
});

beforeAll(() => {
  registerCoreBlocks();
  registerConditionBlocks();
  registerHelperBlocks();
  registerDeviceBlocks();
});

describe('quick add search', () => {
  const entries = [entry('Turn off light'), entry('Turn on light'), entry('Toggle light'), entry('wait 5 seconds', 'Actions')];

  it('needs every word', () => {
    expect(search(entries, 'turn off').map((e) => e.label)).toEqual(['Turn off light']);
    expect(search(entries, 'turn zebra')).toEqual([]);
  });

  it('ranks matches at the start of the label first', () => {
    expect(search(entries, 'light')[0].label).not.toBe('wait 5 seconds');
    expect(search(entries, 'wait')[0].label).toBe('wait 5 seconds');
    expect(search(entries, 'tog')[0].label).toBe('Toggle light');
  });

  it('puts hand-made blocks above generated and technical ones', () => {
    const list = [
      entry('perform action light.turn_on on …', 'Advanced', { tier: 0 }),
      entry('turn on light on …', 'Home Assistant › Light', { tier: 1 }),
      entry('turn on … fade 0 seconds', 'Lights & colours', { tier: 2 }),
    ];
    expect(search(list, 'turn on light').map((e) => e.path)).toEqual(['Lights & colours', 'Home Assistant › Light', 'Advanced']);
  });

  it('finds blocks by other words for the same thing', () => {
    const list = [
      entry('⚡ … becomes on for 0 seconds', 'Events', { tier: 2 }),
      entry('send notification … titled …', 'Home', { tier: 2 }),
      entry('turn on … fade 0 seconds', 'Lights & colours', { tier: 2 }),
      entry('wait 5 seconds', 'Control', { tier: 2 }),
    ];
    expect(search(list, 'motion')[0].label).toContain('becomes');
    expect(search(list, 'notify')[0].label).toContain('notification');
    expect(search(list, 'noti')[0].label).toContain('notification');
    expect(search(list, 'lamp')[0].path).toBe('Lights & colours');
    // A synonym has to be a word of the label, not part of a type or category name.
    expect(search(list, 'delay').map((e) => e.label)).toEqual(['wait 5 seconds']);
  });

  it('ranks what fits the selected block first', () => {
    const list = [
      entry('light is on', 'Sensing', { tier: 2, shape: 'value', output: ['Boolean'] }),
      entry('turn on light', 'Home', { tier: 2, shape: 'statement' }),
      entry('⚡ light turned on', 'Events', { tier: 2, shape: 'trigger' }),
    ];
    expect(search(list, 'light', 40, { shape: 'trigger' })[0].shape).toBe('trigger');
    expect(search(list, 'light', 40, { shape: 'statement' })[0].shape).toBe('statement');
    expect(search(list, 'light', 40, { shape: 'value', check: ['Boolean'] })[0].shape).toBe('value');
    // A statement with an empty socket: still statements first, then values that fit the socket.
    const shapes = (check: string[] | null | undefined) => search(list, 'light', 40, { shape: 'statement', check }).map((e) => e.shape);
    expect(shapes(['Boolean'])).toEqual(['statement', 'value', 'trigger']);
    expect(shapes(['Number'])).toEqual(search(list, 'light', 40, { shape: 'statement' }).map((e) => e.shape));
  });

  it('still prefers an action that matches better while a trigger is selected', () => {
    const list = [
      entry('⚡ light turned on for … Trigger when any of them', 'Home Assistant › Light', { tier: 1, shape: 'trigger' }),
      entry('turn on … fade 0 seconds', 'Lights & colours', { tier: 2, shape: 'statement' }),
      entry('turn on light on …', 'Home Assistant › Light', { tier: 1, shape: 'statement' }),
    ];
    expect(search(list, 'turn on light', 40, { shape: 'trigger' }).map((e) => e.shape)).toEqual(['statement', 'statement', 'trigger']);
    expect(search(list, 'light', 40, { shape: 'trigger' })[0].shape).toBe('trigger');
  });
});

describe('quick add entries', () => {
  const entries = () => toolboxEntries(buildToolbox());

  it('leave hidden fields and templates out of labels', () => {
    for (const e of entries()) expect(e.label, e.state.type).not.toMatch(/\{\{|\{%/);
    const numeric = entries().find((e) => e.state.type === 'ha_trigger_numeric');
    expect(numeric?.label).toMatch(/is above 25 and below/);
  });

  it('know how blocks plug in', () => {
    const byType = (t: string) => entries().find((e) => e.state.type === t);
    expect(byType('ha_trigger_becomes')?.shape).toBe('trigger');
    expect(byType('ha_wait')?.shape).toBe('statement');
    expect(byType('ha_when')?.shape).toBe('top');
    expect(byType('ha_when')?.tier).toBe(2);
    expect(byType('ha_action_raw')?.tier).toBe(0);
  });

  it('rank the everyday blocks first', () => {
    const top = (q: string) => search(entries(), q)[0]?.state.type;
    expect(top('wait')).toBe('ha_wait');
    expect(top('motion')).toBe('ha_trigger_becomes');
    expect(search(entries(), 'yaml').every((e) => e.tier === 0)).toBe(true);
  });
});

describe('quick add placement', () => {
  const make = (ws: Blockly.Workspace, state: Blockly.serialization.blocks.State) =>
    Blockly.serialization.blocks.append(state, ws, { recordUndo: false });

  it('puts triggers into a "when" and actions at the end of its "do"', () => {
    const ws = new Blockly.Workspace();
    const when = make(ws, { type: 'ha_when', inputs: { DO: { block: { type: 'ha_wait', id: 'w1' } } } });
    expect(wantFor(when)).toEqual({ shape: 'trigger' });
    const t1 = make(ws, { type: 'ha_trigger_becomes' });
    expect(attach(when, t1)).toBe(true);
    expect(when.getInputTargetBlock('TRIGGERS')).toBe(t1);
    expect(wantFor(when)).toEqual({ shape: 'statement' });
    expect(wantFor(t1)?.shape).toBe('trigger');

    const t2 = make(ws, { type: 'ha_trigger_becomes' });
    expect(attach(t1, t2)).toBe(true);
    expect(t1.getNextBlock()).toBe(t2);

    // An action chosen while a trigger is selected goes after the last action.
    const w2 = make(ws, { type: 'ha_wait' });
    expect(attach(t1, w2)).toBe(true);
    expect(ws.getBlockById('w1')?.getNextBlock()).toBe(w2);
  });

  it('puts a trigger into an empty "when" when nothing is selected', () => {
    const ws = new Blockly.Workspace();
    const when = make(ws, { type: 'ha_when' });
    expect(wantFor(when)).toEqual({ shape: 'trigger' });
    const t = make(ws, { type: 'ha_trigger_becomes' });
    expect(intoEmptyWhen(ws, t)).toBe(true);
    expect(when.getInputTargetBlock('TRIGGERS')).toBe(t);
    expect(intoEmptyWhen(ws, make(ws, { type: 'ha_trigger_becomes' }))).toBe(false);
  });

  it('puts a value into the first empty socket that fits', () => {
    const ws = new Blockly.Workspace();
    const cmp = make(ws, { type: 'logic_compare' });
    expect(wantFor(cmp)?.shape).toBe('value');
    const n = make(ws, { type: 'math_number' });
    expect(attach(cmp, n)).toBe(true);
    expect(cmp.getInputTargetBlock('A')).toBe(n);
    const m = make(ws, { type: 'math_number' });
    expect(attach(cmp, m)).toBe(true);
    expect(cmp.getInputTargetBlock('B')).toBe(m);
    expect(attach(cmp, make(ws, { type: 'math_number' }))).toBe(false);
  });
});
