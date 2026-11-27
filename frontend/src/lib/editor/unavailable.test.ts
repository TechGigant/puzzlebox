// @vitest-environment jsdom
import * as Blockly from 'blockly/core';
import 'blockly/blocks';
import * as En from 'blockly/msg/en';
import { beforeAll, describe, expect, it } from 'vitest';
import { countTypes, registerUnavailableBlock, typeLabel, unwrapUnavailable, wrapUnavailable, UNAVAILABLE } from './unavailable';

beforeAll(() => {
  Blockly.setLocale(En as unknown as Record<string, string>);
  registerUnavailableBlock();
});

const known = (t: string) => !!Blockly.Blocks[t];
const why = (t: string) => ({ reason: t.startsWith('z2m__') ? "Zigbee2MQTT isn't connected" : "Home Assistant isn't connected", pending: true });

/** Loads a saved workspace the way the editor does and saves it again. */
function roundTrip(saved: object, ws = new Blockly.Workspace()) {
  const copy = JSON.parse(JSON.stringify(saved));
  const missing = wrapUnavailable(copy, known, why, (n) => typeLabel(n.type));
  ws.clear();
  Blockly.serialization.workspaces.load(copy, ws);
  return { saved: unwrapUnavailable(Blockly.serialization.workspaces.save(ws)), missing, ws };
}

/** Blockly's own saved form of a workspace with only built-in blocks. */
function canonical(state: object): Record<string, unknown> {
  const ws = new Blockly.Workspace();
  Blockly.serialization.workspaces.load(state, ws);
  const out = Blockly.serialization.workspaces.save(ws);
  ws.dispose();
  return out as Record<string, unknown>;
}

const print = (id: string, text: string) => ({ type: 'text_print', id, inputs: { TEXT: { block: { type: 'text', id: `${id}t`, fields: { TEXT: text } } } } });

describe('blocks that are not available', () => {
  it('round-trip byte for byte, with what is inside them and after them', () => {
    const base = canonical({
      blocks: {
        languageVersion: 0,
        blocks: [
          {
            type: 'controls_repeat_ext',
            id: 'r',
            x: 10,
            y: 20,
            inputs: { TIMES: { shadow: { type: 'math_number', id: 'n', fields: { NUM: 3 } } }, DO: { block: print('a', 'after') } },
          },
          { ...print('b', 'top'), x: 300, y: 40 },
          { ...print('c', 'value goes here'), x: 300, y: 200 },
        ],
      },
      variables: [{ name: 'count', id: 'v1' }],
    }) as { blocks: { blocks: Record<string, any>[] }; variables: unknown[] };
    const [repeat, top, valueHolder] = base.blocks.blocks;
    // A Home Assistant action inside the loop, with a block inside it and one after it.
    repeat.inputs.DO.block = {
      type: 'ha_svc__light__turn_on',
      id: 'u1',
      extraState: { advanced: true },
      fields: { MODE: 'x' },
      inputs: {
        TARGET: { block: { type: 'ha_area', id: 'u1a', fields: { ID: 'kitchen' } } },
        P_brightness_pct: { shadow: { type: 'math_number', id: 'u1n', fields: { NUM: 50 } } },
      },
      next: { block: repeat.inputs.DO.block },
    };
    // A value block from Home Assistant in a socket.
    valueHolder.inputs.TEXT.block = { type: 'ha_cnd__light__is_on', id: 'u2', fields: { BEHAVIOR: 'any' } };
    // A Zigbee2MQTT block on its own on the canvas, with known blocks under it.
    const z2m = { type: 'z2m__zigbee2mqtt__lamp__set', id: 'u3', x: 600, y: 10, fields: { STATE: 'ON' }, next: { block: top } };
    delete top.x;
    delete top.y;
    base.blocks.blocks[1] = z2m;
    const before = JSON.stringify(base);

    const first = roundTrip(base);
    expect(JSON.stringify(first.saved)).toBe(before);
    expect(first.missing.map((m) => m.type).sort()).toEqual(['ha_cnd__light__is_on', 'ha_svc__light__turn_on', 'z2m__zigbee2mqtt__lamp__set']);
    // The blocks after the missing ones are real blocks, not hidden.
    expect(first.ws.getBlockById('a')?.getPreviousBlock()?.type).toBe(UNAVAILABLE);
    expect(first.ws.getBlockById('b')?.getPreviousBlock()?.id).toBe('u3');
    expect(first.ws.getBlockById('u1a')).toBeNull();
    // ...and again: nothing creeps in.
    expect(JSON.stringify(roundTrip(first.saved).saved)).toBe(before);
    // Once the blocks exist again, the same save loads into them.
    expect(base).not.toHaveProperty('blocks.blocks.0.inputs.DO.block.extraState.original');
  });

  it('keep their place when moved and are counted for the save check', () => {
    const saved = canonical({ blocks: { languageVersion: 0, blocks: [{ ...print('p', 'x'), x: 0, y: 0 }] } }) as { blocks: { blocks: Record<string, any>[] } };
    saved.blocks.blocks[0].next = { block: { type: 'ha_svc__light__turn_off', id: 'u', fields: {} } };
    const { ws } = roundTrip(saved);
    const ph = ws.getBlockById('u')!;
    ph.unplug();
    ph.moveBy(50, 60);
    const out = unwrapUnavailable(Blockly.serialization.workspaces.save(ws)) as { blocks: { blocks: Record<string, any>[] } };
    const moved = out.blocks.blocks.find((b) => b.id === 'u')!;
    expect(Object.keys(moved)).toEqual(['type', 'id', 'x', 'y', 'fields']);
    expect(moved.type).toBe('ha_svc__light__turn_off');
    const types = new Set(['ha_svc__light__turn_off']);
    expect(countTypes(out, types).get('ha_svc__light__turn_off')).toBe(1);
    ph.dispose(false);
    expect(countTypes(unwrapUnavailable(Blockly.serialization.workspaces.save(ws)), types).size).toBe(0);
  });

  it('read like the block they stand for', () => {
    expect(typeLabel('ha_svc__light__turn_on')).toBe('Turn on light');
    expect(typeLabel('ha_trg__light__turned_on')).toBe('Light turned on');
    expect(typeLabel('pack__mypack__do_it')).toBe('Mypack do it');
  });
});
