// @vitest-environment jsdom
import * as Blockly from 'blockly/core';
import 'blockly/blocks';
import { beforeAll, describe, expect, it } from 'vitest';
import {
  attach,
  connectTo,
  freeSpot,
  hasEmptyWhen,
  intoEmptyWhen,
  prefill,
  queryNumber,
  search,
  toolboxEntries,
  wantFor,
  wantForConnection,
  type Entry,
} from './quickadd';
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

  it('ranks blocks that match more of the words first', () => {
    expect(search(entries, 'turn off')[0].label).toBe('Turn off light');
    expect(search(entries, 'turn off').map((e) => e.label)).toContain('Turn on light');
    expect(search(entries, 'turn zebra').map((e) => e.label)).toEqual(['Turn on light', 'Turn off light']);
    expect(search(entries, 'zebra')).toEqual([]);
    // At least half of the words have to match.
    expect(search(entries, 'turn zebra giraffe')).toEqual([]);
  });

  it('ignores numbers, which are filled in instead', () => {
    expect(search(entries, 'wait 10').map((e) => e.label)).toEqual(['wait 5 seconds']);
    expect(search(entries, '10')).toEqual([]);
    expect(queryNumber('wait 10')).toBe(10);
    expect(queryNumber('set to 21,5 degrees')).toBe(21.5);
    expect(queryNumber('wait')).toBeNull();
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

  it('puts triggers first for an empty "when"', () => {
    const list = [
      entry('open …', 'Home', { tier: 2, shape: 'statement' }),
      entry('⚡ … becomes on for 0 seconds', 'Events', { tier: 2, shape: 'trigger' }),
    ];
    expect(search(list, 'door open')[0].shape).toBe('statement');
    expect(search(list, 'door open', 40, { shape: 'trigger', slot: true })[0].shape).toBe('trigger');
    // Unless an action matches more of the words.
    const more = [...list, entry('open the door', 'Home', { tier: 2, shape: 'statement' })];
    expect(search(more, 'open the door', 40, { shape: 'trigger', slot: true })[0].label).toBe('open the door');
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
    expect(top('notify')).toBe('notify_send');
    expect(top('repeat every')).toBe('ha_trigger_every');
    expect(search(entries(), 'for 2 minutes').map((e) => e.state.type)).toContain('device_on_for');
    expect(search(entries(), 'door open', 40, { shape: 'trigger', slot: true })[0].state.type).toBe('ha_trigger_becomes');
    expect(search(entries(), 'yaml').every((e) => e.tier === 0)).toBe(true);
  });
});

describe('quick add placement', () => {
  const make = (ws: Blockly.Workspace, state: Blockly.serialization.blocks.State) =>
    Blockly.serialization.blocks.append(state, ws, { recordUndo: false });

  it('puts triggers into a "when" and actions at the end of its "do"', () => {
    const ws = new Blockly.Workspace();
    const when = make(ws, { type: 'ha_when', inputs: { DO: { block: { type: 'ha_wait', id: 'w1' } } } });
    expect(wantFor(when)).toEqual({ shape: 'trigger', slot: true });
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
    expect(wantFor(when)).toEqual({ shape: 'trigger', slot: true });
    expect(hasEmptyWhen(ws)).toBe(true);
    const t = make(ws, { type: 'ha_trigger_becomes' });
    expect(intoEmptyWhen(ws, t)).toBe(true);
    expect(when.getInputTargetBlock('TRIGGERS')).toBe(t);
    expect(intoEmptyWhen(ws, make(ws, { type: 'ha_trigger_becomes' }))).toBe(false);
    expect(hasEmptyWhen(ws)).toBe(false);
  });

  it('plugs into the connection the keyboard cursor is on', () => {
    const ws = new Blockly.Workspace();
    const when = make(ws, { type: 'ha_when', inputs: { DO: { block: { type: 'ha_wait', id: 'w1' } } } });
    const triggers = when.getInput('TRIGGERS')!.connection!;
    expect(wantForConnection(triggers)).toEqual({ shape: 'trigger', slot: true });
    // An action can't go into the trigger slot.
    expect(connectTo(triggers, make(ws, { type: 'ha_log' }))).toBe(false);
    // Into the "do", in front of what is there.
    const body = when.getInput('DO')!.connection!;
    expect(wantForConnection(body)).toEqual({ shape: 'statement' });
    const log = make(ws, { type: 'ha_log' });
    expect(connectTo(body, log)).toBe(true);
    expect(when.getInputTargetBlock('DO')).toBe(log);
    expect(log.getNextBlock()?.id).toBe('w1');
    // Above a block: between it and the one before.
    const log2 = make(ws, { type: 'ha_log' });
    expect(connectTo(ws.getBlockById('w1')!.previousConnection!, log2)).toBe(true);
    expect(log.getNextBlock()).toBe(log2);
    expect(log2.getNextBlock()?.id).toBe('w1');
    // A value into a socket.
    const cmp = make(ws, { type: 'logic_compare' });
    const a = cmp.getInput('A')!.connection!;
    expect(wantForConnection(a)?.shape).toBe('value');
    const n = make(ws, { type: 'math_number' });
    expect(connectTo(a, n)).toBe(true);
    expect(cmp.getInputTargetBlock('A')).toBe(n);
  });

  it('fills a number from the query into the block', () => {
    const ws = new Blockly.Workspace();
    const wait = make(ws, { type: 'ha_wait', inputs: { SECONDS: { shadow: { type: 'duration', inputs: { N: { shadow: { type: 'math_number', fields: { NUM: 5 } } } } } } } });
    expect(prefill(wait, 'wait 2 minutes')).toBe(true);
    const d = wait.getInputTargetBlock('SECONDS')!;
    expect(d.getInputTargetBlock('N')!.getFieldValue('NUM')).toBe(2);
    expect(d.getFieldValue('UNIT')).toBe('minutes');
    const every = make(ws, { type: 'ha_trigger_every' });
    expect(prefill(every, 'every 10 sec')).toBe(true);
    expect(every.getFieldValue('N')).toBe(10);
    expect(every.getFieldValue('UNIT')).toBe('seconds');
    expect(prefill(make(ws, { type: 'ha_log' }), 'log 5')).toBe(false);
    expect(prefill(every, 'every minute')).toBe(false);
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

describe('free spot', () => {
  const box = (left: number, top: number, w: number, h: number) => ({ left, top, right: left + w, bottom: top + h });
  const xy = (c: Blockly.utils.Coordinate) => [c.x, c.y];

  it('is the mouse when nothing is there', () => {
    expect(xy(freeSpot([box(0, 0, 100, 50)], 80, 40, { x: 300, y: 300 }))).toEqual([300, 300]);
  });

  it('is next to the block under the mouse, on the nearer side', () => {
    // A wide, short block: going below it is shorter than going round it.
    expect(xy(freeSpot([box(0, 0, 400, 60)], 80, 40, { x: 150, y: 10 }))).toEqual([150, 80]);
    // A tall, narrow one: going right is shorter.
    expect(xy(freeSpot([box(0, 0, 100, 600)], 80, 40, { x: 70, y: 300 }))).toEqual([120, 300]);
  });

  it('avoids the neighbours of that spot too', () => {
    const taken = [box(0, 0, 400, 60), box(0, 70, 400, 60)];
    // Below the first is taken by the second, so above it is nearest.
    const at = freeSpot(taken, 80, 40, { x: 150, y: 10 });
    expect(taken.every((r) => at.y >= r.bottom || at.y + 40 <= r.top || at.x >= r.right || at.x + 80 <= r.left)).toBe(true);
    expect(xy(at)).toEqual([150, -60]);
  });
});
