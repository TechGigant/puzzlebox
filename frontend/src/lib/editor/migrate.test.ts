import { describe, expect, it, vi } from 'vitest';

vi.mock('../blocks/dynamic', () => ({
  blockDef: (type: string) =>
    type === 'ha_svc__light__turn_on' ? { params: [{ name: 'rgb_color', input: 'color' }] } : undefined,
}));

const { migrateWorkspace } = await import('./migrate');

const ws = (...blocks: object[]) => ({ blocks: { languageVersion: 0, blocks } });

describe('migrateWorkspace', () => {
  it('moves entity fields into sockets and splits mixed blocks', () => {
    const out = migrateWorkspace(
      ws(
        { type: 'ha_state', fields: { ENTITY: 'sensor.t' } },
        { type: 'ha_state_check', fields: { MODE: 'count', OP: 'is', STATE: 'on' } },
        { type: 'ha_history', fields: { OP: 'state_ago', STATE: '' }, inputs: { SECONDS: { shadow: { type: 'math_number', fields: { NUM: 60 } } } } },
      ),
    ) as { blocks: { blocks: Record<string, unknown>[] } };
    const [state, check, history] = out.blocks.blocks;
    expect(state).toEqual({ type: 'ha_state', fields: {}, inputs: { ENTITY: { shadow: { type: 'ha_entity', fields: { ENTITY: 'sensor.t' } } } } });
    expect(check.type).toBe('ha_state_count');
    expect(history.type).toBe('ha_state_ago');
  });

  it('turns trigger numbers and seconds into durations, and colours into sockets', () => {
    const out = migrateWorkspace(
      ws(
        { type: 'ha_trigger_state', fields: { ENTITY: 'light.a', FOR: 30 } },
        { type: 'ha_wait', inputs: { SECONDS: { shadow: { type: 'math_number', fields: { NUM: 5 } } } } },
        { type: 'ha_svc__light__turn_on', fields: { P_rgb_color: '#ff0000' } },
      ),
    ) as { blocks: { blocks: { inputs: Record<string, { shadow: { type: string; fields?: object } }> }[] } };
    const [trigger, wait, light] = out.blocks.blocks;
    expect(trigger.inputs.FOR.shadow.type).toBe('duration');
    expect(wait.inputs.SECONDS.shadow.type).toBe('duration');
    expect(light.inputs.P_rgb_color.shadow).toEqual({ type: 'colour_picker', fields: { COLOUR: '#ff0000' } });
  });
});

describe('Zigbee2MQTT group units', () => {
  it('converts 0–254 brightness and mired to % and K', () => {
    const out = migrateWorkspace(
      ws({
        type: 'z2m__zigbee2mqtt__group3__set',
        extraState: { optional: ['brightness', 'color_temp'] },
        inputs: {
          P_brightness: { shadow: { type: 'math_number', fields: { NUM: 254 } } },
          P_color_temp: { block: { type: 'variables_get', fields: { VAR: { id: 'v' } } } },
        },
      }),
    ) as { blocks: { blocks: { extraState: { optional: string[] }; inputs: Record<string, { shadow?: { fields: { NUM: number } }; block?: { type: string } }> }[] } };
    const [g] = out.blocks.blocks;
    expect(g.extraState.optional).toEqual(['brightness_percent', 'color_temp_kelvin']);
    expect(g.inputs.P_brightness_percent.shadow?.fields.NUM).toBe(100);
    expect(g.inputs.P_color_temp_kelvin.block?.type).toBe('math_arithmetic');
    expect(g.inputs.P_brightness).toBeUndefined();
  });
});
