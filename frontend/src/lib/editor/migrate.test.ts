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
