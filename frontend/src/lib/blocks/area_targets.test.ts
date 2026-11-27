// @vitest-environment jsdom
import * as Blockly from 'blockly/core';
import 'blockly/blocks';
import { beforeAll, describe, expect, it, vi } from 'vitest';

vi.mock('../api', () => ({
  getJson: vi.fn(async (path: string) => {
    if (path.startsWith('api/ha/states')) {
      const changed = [
        { entity_id: 'binary_sensor.kitchen_motion', state: 'off', attributes: { device_class: 'motion', friendly_name: 'Kitchen motion' } },
        { entity_id: 'binary_sensor.kitchen_leak', state: 'off', attributes: { device_class: 'moisture', friendly_name: 'Kitchen leak' } },
        { entity_id: 'binary_sensor.hall_leak', state: 'off', attributes: { device_class: 'moisture', friendly_name: 'Hall leak' } },
      ];
      return { full: true, version: 1, changed, removed: [] };
    }
    if (path === 'api/ha/registry') {
      return {
        areas: [
          { area_id: 'kitchen', name: 'Kitchen' },
          { area_id: 'hall', name: 'Hall' },
        ],
        floors: [],
        labels: [],
        devices: [],
        entities: [
          { entity_id: 'binary_sensor.kitchen_motion', area_id: 'kitchen' },
          { entity_id: 'binary_sensor.kitchen_leak', area_id: 'kitchen' },
          { entity_id: 'binary_sensor.hall_leak', area_id: 'hall' },
        ],
      };
    }
    return [];
  }),
}));

import { registerCoreBlocks } from './core';
import { installRegistry } from './dynamic';
import { loadEntities, loadRegistry } from './field_entity';

beforeAll(async () => {
  registerCoreBlocks();
  installRegistry([
    {
      type: 'ha_trg__motion__detected',
      kind: 'trigger',
      category: ['Motion'],
      style: 'trigger',
      title: 'Motion detected',
      target: { domains: ['binary_sensor'], device_classes: ['motion'], required: true },
      params: [],
      source: 'ha',
    },
  ]);
  await loadEntities(true);
  await loadRegistry(true);
});

describe('area targets with a device class', () => {
  it('offer only areas with such sensors, named by the class', () => {
    // HA (checked live on 2026.11) fires motion.detected with target {area_id} only for the
    // area's motion sensors, so "All motion sensors in Kitchen" says what it does.
    const ws = new Blockly.Workspace();
    const trigger = Blockly.serialization.blocks.append(
      { type: 'ha_trg__motion__detected', inputs: { TARGET: { shadow: { type: 'ha_entity', fields: { ENTITY: '' } } } } },
      ws,
    );
    const field = trigger.getInputTargetBlock('TARGET')!.getField('ENTITY') as unknown as {
      suggestions(q: string): { value: string; label: string }[];
    };
    const found = field.suggestions('');
    expect(found.map((s) => s.label)).toEqual(['All motion sensors in Kitchen', 'Kitchen motion']);
  });
});
