import { describe, expect, it, vi } from 'vitest';

vi.mock('../blocks/dynamic', () => ({
  blockDef: (type: string) =>
    type === 'ha_svc__light__turn_on'
      ? { params: [{ name: 'rgb_color', input: 'color' }, { name: 'transition', input: 'duration' }] }
      : type === 'z2m__zigbee2mqtt__0x000d6ffffe112233__set'
        ? { params: [{ name: 'brightness_percent', input: 'number' }, { name: 'color_temp_kelvin', input: 'number' }] }
        : type === 'z2m__zigbee2mqtt__0x01__set'
          ? { params: [{ name: 'brightness', input: 'number' }] }
          : undefined,
  secondsParam: (p: { input: string }) => p.input === 'duration',
  durationShadow: (n: number) => ({ type: 'duration', fields: { UNIT: 'seconds' }, inputs: { N: { shadow: { type: 'math_number', fields: { NUM: n } } } } }),
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
        { type: 'ha_svc__light__turn_on', fields: { P_rgb_color: '#ff0000' }, inputs: { P_transition: { shadow: { type: 'math_number', fields: { NUM: 3 } } } } },
      ),
    ) as { blocks: { blocks: { inputs: Record<string, { shadow: { type: string; fields?: object } }> }[] } };
    const [trigger, wait, light] = out.blocks.blocks;
    expect(trigger.inputs.FOR.shadow.type).toBe('duration');
    expect(wait.inputs.SECONDS.shadow.type).toBe('duration');
    expect(light.inputs.P_rgb_color.shadow).toEqual({ type: 'colour_picker', fields: { COLOUR: '#ff0000' } });
    expect(light.inputs.P_transition.shadow).toEqual({ type: 'duration', fields: { UNIT: 'seconds' }, inputs: { N: { shadow: { type: 'math_number', fields: { NUM: 3 } } } } });
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

  it('converts device brightness to % (exactly) and white temperature to K', () => {
    type Out = { blocks: { blocks: { extraState: { optional: string[] }; inputs: Record<string, { shadow?: { fields: { NUM: number } }; block?: { type: string } }> }[] } };
    const out = migrateWorkspace(
      ws(
        {
          type: 'z2m__zigbee2mqtt__0x000d6ffffe112233__set',
          fields: { P_state: 'ON' },
          extraState: { optional: ['brightness', 'color_temp'] },
          inputs: {
            P_brightness: { shadow: { type: 'math_number', fields: { NUM: 128 } } },
            P_color_temp: { shadow: { type: 'math_number', fields: { NUM: 370 } } },
          },
        },
        {
          type: 'z2m__zigbee2mqtt__0x000d6ffffe112233__set',
          inputs: { P_brightness: { block: { type: 'variables_get', fields: { VAR: { id: 'v' } } } } },
        },
        // A device whose brightness isn't 0–254 keeps it.
        { type: 'z2m__zigbee2mqtt__0x01__set', inputs: { P_brightness: { shadow: { type: 'math_number', fields: { NUM: 7 } } } } },
      ),
    ) as Out;
    const [d, calc, other] = out.blocks.blocks;
    expect(d.extraState.optional).toEqual(['brightness_percent', 'color_temp_kelvin']);
    expect(d.inputs.P_brightness_percent.shadow?.fields.NUM).toBe(50.39);
    expect(Math.round(50.39 * 2.54)).toBe(128);
    expect(d.inputs.P_color_temp_kelvin.shadow?.fields.NUM).toBe(2703);
    expect(d.inputs.P_brightness).toBeUndefined();
    expect(calc.inputs.P_brightness_percent.block?.type).toBe('math_arithmetic');
    expect(other.inputs.P_brightness.shadow?.fields.NUM).toBe(7);
  });

  it('gives older triggers the new duration sockets, set to nothing', () => {
    const out = migrateWorkspace(
      ws(
        { type: 'ha_trigger_numeric', fields: { ENTITY: 'sensor.t', ABOVE: '25', BELOW: '' } },
        { type: 'ha_trigger_time', fields: { AT: '07:00' }, inputs: { OFFSET: { block: { type: 'math_number', fields: { NUM: 60 } } } } },
      ),
    ) as { blocks: { blocks: { inputs: Record<string, { block?: { fields: object }; shadow?: { type: string; fields: object } }> }[] } };
    const [numeric, time] = out.blocks.blocks;
    expect(numeric.inputs.FOR.shadow?.type).toBe('duration');
    expect(numeric.inputs.FOR.shadow?.fields).toEqual({ UNIT: 'seconds' });
    expect(time.inputs.OFFSET.block?.fields).toEqual({ NUM: 60 });
  });
});

describe('wait time limits', () => {
  type Wait = { type: string; fields: Record<string, unknown>; inputs?: Record<string, { block?: Record<string, any>; shadow?: Record<string, any> }> };
  const secs = (n: number) => ({ shadow: { type: 'math_number', fields: { NUM: n } } });
  const minutes = (n: number) => ({ shadow: { type: 'duration', fields: { UNIT: 'minutes' }, inputs: { N: { shadow: { type: 'math_number', fields: { NUM: n } } } } } });

  it('keeps an old time of 0 meaning no limit', () => {
    const out = migrateWorkspace(
      ws(
        { type: 'ha_wait_for_trigger', inputs: { TIMEOUT: secs(0) } },
        { type: 'ha_wait_for_trigger', inputs: { TIMEOUT: minutes(10) } },
        { type: 'wait_until', inputs: { TIMEOUT: minutes(0) } },
        { type: 'wait_until', inputs: {} },
        { type: 'wait_until', fields: { LIMIT: 'limit' }, inputs: { TIMEOUT: secs(0) } },
      ),
    ) as { blocks: { blocks: Wait[] } };
    expect(out.blocks.blocks.map((b) => b.fields.LIMIT)).toEqual(['none', 'limit', 'none', 'none', 'limit']);
  });

  it('gives a worked-out time no limit when it comes to 0', () => {
    const v = { type: 'variables_get', id: 'g1', fields: { VAR: { id: 'v' } } };
    const out = migrateWorkspace(ws({ type: 'wait_until', inputs: { TIMEOUT: { ...secs(5), block: v } } })) as { blocks: { blocks: Wait[] } };
    const [w] = out.blocks.blocks;
    expect(w.fields.LIMIT).toBe('limit');
    const t = w.inputs!.TIMEOUT;
    expect(t.shadow?.type).toBe('math_number');
    expect(t.block?.type).toBe('logic_ternary');
    expect(t.block?.inputs.IF.block.fields.OP).toBe('GT');
    expect(t.block?.inputs.IF.block.inputs.A.block).toEqual({ type: 'variables_get', fields: { VAR: { id: 'v' } } });
    expect(t.block?.inputs.THEN.block).toBe(v);
    expect(t.block?.inputs.ELSE.block.type).toBe('logic_null');
  });
});
