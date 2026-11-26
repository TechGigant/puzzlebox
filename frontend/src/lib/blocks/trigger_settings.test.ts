// @vitest-environment jsdom
import * as Blockly from 'blockly/core';
import 'blockly/blocks';
import { beforeAll, describe, expect, it } from 'vitest';
import { registerCoreBlocks } from './core';
import { registerConditionBlocks } from './conditions';
import { registerHelperBlocks } from './helpers';
import { registerDeviceBlocks } from './devices';
import { installRegistry, type BlockDef } from './dynamic';
import { chooseTriggerSetting, hasTriggerSettings } from './trigger_settings';
import { alternatives } from '../editor/switching';
import { applyConversion, conversion } from '../editor/integration_switch';

const generated = (type: string, kind: 'trigger' | 'condition', params: BlockDef['params']): BlockDef => ({
  type,
  kind,
  category: ['Light'],
  style: kind,
  title: kind === 'trigger' ? 'Light turned on' : 'Light is on',
  target: { domains: ['light'], required: true },
  params,
  source: 'ha',
});

const behavior = (mode: string): BlockDef['params'][number] => ({
  name: 'behavior',
  label: 'Behavior',
  required: true,
  input: 'select',
  options: [
    { label: 'Any', value: 'any' },
    { label: 'Each', value: 'each' },
    { label: 'All', value: mode },
  ],
});

beforeAll(() => {
  registerCoreBlocks();
  registerConditionBlocks();
  registerHelperBlocks();
  registerDeviceBlocks();
  installRegistry([
    generated('ha_trg__light__turned_on', 'trigger', [behavior('first'), { name: 'for', label: 'For', input: 'duration' }]),
    generated('ha_cnd__light__brightness', 'condition', [
      { name: 'threshold', label: 'Threshold', required: true, input: 'threshold', mode: 'is', units: [] },
    ]),
  ]);
});

const load = (state: Blockly.serialization.blocks.State, ws = new Blockly.Workspace()) => Blockly.serialization.blocks.append(state, ws);
const save = (b: Blockly.Block) => Blockly.serialization.blocks.save(b, { addCoordinates: false, addInputBlocks: false });
const settle = () => new Promise((r) => setTimeout(r));

describe('trigger id and name', () => {
  it('saves and loads the settings with their values', () => {
    const b = load({ type: 'ha_trigger_becomes', extraState: { triggerSettings: ['TRIGGER_ID', 'TRIGGER_ALIAS'] }, fields: { ENTITY: 'light.a', TRIGGER_ID: 'door', TRIGGER_ALIAS: 'Door opened' } });
    const s = save(b)!;
    expect(s.extraState).toEqual({ triggerSettings: ['TRIGGER_ID', 'TRIGGER_ALIAS'] });
    expect(s.fields).toMatchObject({ TRIGGER_ID: 'door', TRIGGER_ALIAS: 'Door opened' });
  });

  it('keeps old blocks as they were', () => {
    const s = save(load({ type: 'ha_trigger_state', fields: { ENTITY: 'light.a', TO: 'on' } }))!;
    expect(s.extraState).toBeUndefined();
    expect(s.fields).not.toHaveProperty('TRIGGER_ID');
  });

  it('adds and removes rows from the menu', () => {
    const b = load({ type: 'ha_trigger_time', fields: { AT: '07:00' } });
    expect(chooseTriggerSetting(b, '+TRIGGER_ID')).toBe(true);
    b.setFieldValue('morning', 'TRIGGER_ID');
    expect(save(b)!.fields).toMatchObject({ TRIGGER_ID: 'morning' });
    // The menu stays last.
    expect(b.inputList.at(-1)!.name).toBe('TRIGGER_MENU');
    chooseTriggerSetting(b, '-TRIGGER_ID');
    expect(b.getField('TRIGGER_ID')).toBeNull();
    expect(save(b)!.extraState).toBeUndefined();
    expect(chooseTriggerSetting(b, '+SOMETHING')).toBe(false);
  });

  it('is on Home Assistant triggers only', () => {
    for (const t of ['ha_trigger_state', 'ha_trigger_zone', 'ha_trigger_integration', 'device_trigger', 'ha_trg__light__turned_on']) {
      load({ type: t });
      expect(hasTriggerSettings(t), t).toBe(true);
    }
    for (const t of ['engine_trigger_cron', 'ha_trigger_raw']) {
      expect(load({ type: t }).getInput('TRIGGER_MENU'), t).toBeNull();
      expect(hasTriggerSettings(t), t).toBe(false);
    }
  });

  it('keeps device and generated blocks\' own state', () => {
    const device = { config: { trigger: 'device', device_id: 'd', domain: 'light', type: 'turned_on' }, label: 'light: turned on', params: [] };
    const d = load({ type: 'device_trigger', extraState: { ...device, triggerSettings: ['TRIGGER_ID'] }, fields: { TRIGGER_ID: 'x' } });
    expect(save(d)!.extraState).toEqual({ ...device, triggerSettings: ['TRIGGER_ID'] });
    expect(d.getFieldValue('TRIGGER_ID')).toBe('x');
    chooseTriggerSetting(d, '-TRIGGER_ID');
    expect(save(d)!.extraState).toEqual(device);

    const g = load({ type: 'ha_trg__light__turned_on', extraState: { optional: ['for'], triggerSettings: ['TRIGGER_ALIAS'] }, fields: { TRIGGER_ALIAS: 'On' } });
    expect(save(g)!.extraState).toEqual({ optional: ['for'], triggerSettings: ['TRIGGER_ALIAS'] });
    // Generated blocks have the settings in their own menu, and no second one.
    expect(g.getInput('TRIGGER_MENU')).toBeNull();
    expect(g.inputList.at(-1)!.name).toBe('ADD');
  });

  it('shows the name in the "triggered by" menu', async () => {
    const ws = new Blockly.Workspace();
    const t = load({ type: 'ha_trigger_becomes', id: 'tb1', extraState: { triggerSettings: ['TRIGGER_ALIAS'] }, fields: { ENTITY: 'light.a', TRIGGER_ALIAS: 'Door opened' } }, ws);
    const by = load({ type: 'triggered_by', fields: { TRIGGER: 'tb1' } }, ws);
    await settle();
    const options = (by.getField('TRIGGER') as Blockly.FieldDropdown).getOptions(false);
    expect(options).toContainEqual(['Door opened', t.id]);
  });
});

describe('integration blocks', () => {
  const dict = (pairs: [string, Blockly.serialization.blocks.State][]) => {
    let next: Blockly.serialization.blocks.State | undefined;
    for (const [k, v] of pairs.reverse()) {
      next = { type: 'dict_pair', fields: { KEY: k }, inputs: { VALUE: { block: v } }, ...(next ? { next: { block: next } } : {}) };
    }
    return { block: { type: 'dict_create', ...(next ? { inputs: { ITEMS: { block: next } } } : {}) } };
  };
  const text = (s: string) => ({ type: 'text', fields: { TEXT: s } });

  it('switches to the generated block with the options as settings', () => {
    const b = load({
      type: 'ha_trigger_integration',
      fields: { NAME: 'light.turned_on' },
      inputs: { TARGET: { block: { type: 'ha_area', fields: { ID: 'kitchen' } } }, OPTIONS: dict([['behavior', text('any')], ['for', text('00:05:00')]]) },
    });
    const [alt] = alternatives(b);
    expect(alt).toMatchObject({ type: 'ha_trg__light__turned_on', extraState: { optional: ['for'] }, drop: ['OPTIONS'] });
    const c = conversion(b)!;
    const g = load({ type: alt.type, extraState: alt.extraState });
    applyConversion(g, c);
    expect(g.getFieldValue('P_behavior')).toBe('any');
    expect(g.getInputTargetBlock('P_for')!.getFieldValue('NUM')).toBe(300);
  });

  it('only offers switching when everything fits', () => {
    const alts = (name: string, options: ReturnType<typeof dict>, target = true) =>
      alternatives(
        load({
          type: name.includes('brightness') ? 'ha_condition_integration' : 'ha_trigger_integration',
          fields: { NAME: name },
          inputs: { ...(target ? { TARGET: { block: { type: 'ha_entity', fields: { ENTITY: 'light.a' } } } } : {}), OPTIONS: options },
        }),
      );
    expect(alts('light.turned_on', dict([['unknown', text('x')]]))).toEqual([]);
    expect(alts('light.turned_on', dict([['behavior', text('sometimes')]]))).toEqual([]);
    expect(alts('light.turned_on', dict([]), false)).toEqual([]);
    expect(alts('light.turned_off', dict([]))).toEqual([]);
    const threshold = { block: { type: 'yaml_value', fields: { CONFIG: '{"threshold": {"type": "between", "value_min": {"number": 10}, "value_max": {"number": 90}}}' } } };
    const [alt] = alts('light.brightness', threshold);
    expect(alt.type).toBe('ha_cnd__light__brightness');
  });
});
