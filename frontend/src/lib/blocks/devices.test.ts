// @vitest-environment jsdom
import * as Blockly from 'blockly/core';
import 'blockly/blocks';
import { beforeAll, describe, expect, it, vi } from 'vitest';

const remote = (device: string, subtype: string) => ({
  kind: 'trigger',
  label: `"${subtype}" action`,
  config: { trigger: 'device', domain: 'zha', device_id: device, type: 'remote_button_short_press', subtype },
  params: [],
  secondary: false,
});

vi.mock('../api', () => ({
  getJson: vi.fn(async (path: string) => {
    if (path === 'api/ha/registry') {
      return {
        areas: [],
        floors: [],
        labels: [],
        devices: [
          { id: 'bed', name: 'Bedroom remote', area_id: null },
          { id: 'hall', name: 'Hall remote', area_id: null },
          { id: 'plug', name: 'Plug', area_id: null },
        ],
        entities: [],
      };
    }
    const device = decodeURIComponent(path.split('/')[3] ?? '');
    if (device === 'bed' || device === 'hall') return [remote(device, 'turn_on'), remote(device, 'turn_off')];
    return [];
  }),
}));

import { registerCoreBlocks } from './core';
import { registerDeviceBlocks } from './devices';
import { loadRegistry } from './field_entity';

let ws: Blockly.Workspace;
beforeAll(async () => {
  registerCoreBlocks();
  registerDeviceBlocks();
  ws = new Blockly.Workspace();
  await loadRegistry(true);
});

const flush = () => new Promise((r) => setTimeout(r, 20));

type DeviceBlock = Blockly.Block & { device: { config: Record<string, unknown>; label: string }; warnings: (string | null)[] };

const load = (config: Record<string, unknown>, extra: Record<string, unknown> = {}, label = 'zha: remote button') => {
  const b = Blockly.serialization.blocks.append(
    { type: 'device_trigger', extraState: { config, label, params: [], ...extra } },
    ws,
  ) as DeviceBlock;
  // Only rendered blocks show warnings; record them.
  b.warnings = [];
  (b as unknown as { setWarningText: (t: string | null) => void }).setWarningText = (t) => b.warnings.push(t);
  return b;
};

describe('device blocks', () => {
  it('show which device they belong to', async () => {
    const b = load(remote('bed', 'turn_on').config);
    expect(b.getField('DEVICE')!.getText()).toBe('Bedroom remote');
    // The device is saved in the config only.
    const saved = Blockly.serialization.blocks.save(b)!;
    expect(saved.fields ?? {}).not.toHaveProperty('DEVICE');
    expect((saved.extraState as { config: { device_id: string } }).config.device_id).toBe('bed');
    // Blocks made elsewhere (imported) get Home Assistant's own label once it has loaded.
    await flush();
    expect(b.device.label).toBe('"turn_on" action');
    expect(b.toString()).toContain('Bedroom remote');
  });

  it("don't repeat the device name from Home Assistant's label", () => {
    const b = load({ trigger: 'device', domain: 'zha', device_id: 'bed', type: 'x' }, {}, 'Bedroom remote was pressed');
    expect(b.getField('LABEL')!.getText()).toBe('was pressed');
    expect(b.device.label).toBe('Bedroom remote was pressed');
  });

  it('move to the same trigger of another device, keeping their settings', async () => {
    const b = load(remote('bed', 'turn_off').config, { triggerSettings: ['TRIGGER_ID'] });
    b.setFieldValue('evening', 'TRIGGER_ID');
    b.setFieldValue('hall', 'DEVICE');
    await flush();
    expect(b.device.config).toEqual(remote('hall', 'turn_off').config);
    expect(b.getFieldValue('TRIGGER_ID')).toBe('evening');
  });

  it('stay on their device when the other one has no such trigger', async () => {
    const b = load(remote('bed', 'turn_on').config);
    b.setFieldValue('plug', 'DEVICE');
    await flush();
    expect(b.device.config.device_id).toBe('bed');
    expect(b.getFieldValue('DEVICE')).toBe('bed');
    expect(b.warnings.at(-1)).toContain('Plug has no');
  });

  it('warn about devices that are gone', async () => {
    const b = load(remote('gone', 'turn_on').config);
    await flush();
    expect(b.getField('DEVICE')!.getText()).toBe('gone');
    expect(b.warnings.at(-1)).toContain('no longer in Home Assistant');
  });
});
