import * as Blockly from 'blockly/core';
import 'blockly/blocks';
import { beforeAll, describe, expect, it, vi } from 'vitest';

vi.mock('../api', () => ({
  getJson: vi.fn(async (path: string) => {
    if (path.startsWith('api/ha/states')) {
      const changed = [
        { entity_id: 'binary_sensor.hall_motion', state: 'off', attributes: {} },
        { entity_id: 'tts.google', state: 'unknown', attributes: {} },
        // As HA shows them: from scenes.yaml, and made by scene.create.
        { entity_id: 'scene.evening', state: 'unknown', attributes: { entity_id: ['light.a'], id: '17', friendly_name: 'Evening' } },
        { entity_id: 'scene.before', state: 'unknown', attributes: { entity_id: ['light.a'], friendly_name: 'before' } },
      ];
      return { full: true, version: 1, changed, removed: [] };
    }
    if (path === 'api/ha/registry') {
      return { areas: [], floors: [], labels: [], devices: [{ id: 'p', name: 'Pixel 7', area_id: null }], entities: [] };
    }
    return [];
  }),
}));

import { registerCoreBlocks } from './core';
import { notifyLabel, registerHelperBlocks } from './helpers';
import { entityWarnings, sceneSlug } from './checks';
import { loadEntities, loadRegistry } from './field_entity';
import { plainError } from '../ha_errors';

let ws: Blockly.Workspace;
beforeAll(async () => {
  registerCoreBlocks();
  registerHelperBlocks();
  ws = new Blockly.Workspace();
  await loadEntities(true);
  await loadRegistry(true);
});

const load = (state: Blockly.serialization.blocks.State) =>
  Blockly.serialization.blocks.append(state, ws);
const settle = () => new Promise((r) => setTimeout(r, 10));

describe('block text', () => {
  it('leaves out buttons and menus', () => {
    class Menu extends Blockly.FieldDropdown {
      override isSerializable() {
        return false;
      }
    }
    Blockly.Blocks['test_controls'] = {
      init(this: Blockly.Block) {
        this.appendDummyInput()
          .appendField(new Blockly.FieldImage('data:,', 10, 10, 'Add else if'))
          .appendField('if it rains')
          .appendField(new Menu([['＋ option', 'x']]));
      },
    };
    expect(load({ type: 'test_controls' }).toString()).toBe('if it rains');
  });

  it('leaves out hidden fields and inputs', async () => {
    const b = load({ type: 'ha_trigger_numeric', fields: { ENTITY: 'sensor.power', ABOVE: '25' } });
    expect(b.toString()).not.toContain('state.state');
    const time = load({ type: 'ha_trigger_time', fields: { AT: '07:00' }, inputs: { OFFSET: { shadow: { type: 'math_number', fields: { NUM: 0 } } } } });
    await settle();
    expect(time.getInput('OFFSET')!.isVisible()).toBe(false);
    expect(time.toString()).not.toContain('shifted');
  });

  it('shows the time shift for helpers and for saved shifts', async () => {
    const helper = load({ type: 'ha_trigger_time', fields: { AT: 'input_datetime.wake' } });
    const saved = load({ type: 'ha_trigger_time', fields: { AT: '07:00' }, inputs: { OFFSET: { block: { type: 'math_number', fields: { NUM: 60 } } } } });
    await settle();
    expect(helper.getInput('OFFSET')!.isVisible()).toBe(true);
    expect(saved.getInput('OFFSET')!.isVisible()).toBe(true);
  });

  it('reads plainly', () => {
    expect(load({ type: 'climate_set_mode' }).toString()).toMatch(/^set mode of \? to heat/);
    expect(load({ type: 'notify_send', fields: { TO: 'persistent' } }).toString()).toContain('to Home Assistant sidebar');
    expect(notifyLabel('mobile_app_pixel_7')).toBe('📱 Pixel 7');
    expect(notifyLabel('mobile_app_old_phone')).toBe('📱 Old Phone');
  });
});

describe('entity warnings', () => {
  it('flags anything that is not a known entity, a template or "all"', () => {
    const ids = ['hall', 'binary_sensor.hall_motion', 'light.gone', "{{ trigger.entity_id }}", 'all'].map(
      (e) => load({ type: 'ha_entity', fields: { ENTITY: e } }).id,
    );
    const found = entityWarnings(ws).filter((w) => ids.includes(w.block_id)).map((w) => w.message);
    expect(found).toEqual(['"hall" isn\'t an entity: choose one from the list', "light.gone doesn't exist in Home Assistant"]);
  });
});

describe('scene warnings', () => {
  it("flag remembering under the name of the user's own scene", () => {
    const own = load({ type: 'scene_remember', fields: { NAME: ' Evening!' } });
    const ours = load({ type: 'scene_remember', fields: { NAME: 'before' } });
    const found = entityWarnings(ws).filter((w) => [own.id, ours.id].includes(w.block_id));
    expect(found).toEqual([
      { block_id: own.id, message: "There's already a scene called scene.evening in your configuration: choose another name" },
    ]);
    expect(sceneSlug(' Before the Movie! ')).toBe('before_the_movie');
  });
});

describe('say', () => {
  it('starts with the first voice when added, not when loaded', async () => {
    const added = load({ type: 'media_say' });
    const saved = new Blockly.Workspace();
    Blockly.serialization.workspaces.load({ blocks: { languageVersion: 0, blocks: [{ type: 'media_say', id: 'old' }] } }, saved);
    await settle();
    expect(added.getFieldValue('TTS')).toBe('tts.google');
    expect(saved.getBlockById('old')!.getFieldValue('TTS')).toBe('');
  });
});

describe('Home Assistant errors', () => {
  it('reads plainly', () => {
    expect(plainError("Home Assistant returned an error (invalid_format): at least one of ['entity_id', 'device_id', 'area_id', 'floor_id', 'label_id'] is required")).toMatch(/^Nothing to act on/);
    expect(plainError('Home Assistant returned an error (not_found): Service light.nope not found.')).toContain('no action light.nope');
    expect(plainError("value must be at most 100 for dictionary value @ data['brightness_pct']")).toBe('"brightness_pct" can be at most 100.');
    expect(plainError("In 'state' condition: unknown entity light.x")).toContain("doesn't know light.x");
    expect(plainError('The scene scene.evening already exists')).toBe(
      "There's already a scene called scene.evening in your configuration: choose another name to remember the state as.",
    );
    expect(plainError('something else')).toBeNull();
  });
});
