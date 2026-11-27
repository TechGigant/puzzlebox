//! Starter programs for the empty program list. Entities are left to choose, so each opens
//! as a new, unsaved program that compiles once its pickers are filled in.

import type { RunMode } from './api';

type State = Record<string, unknown>;

const num = (n: number) => ({ shadow: { type: 'math_number', fields: { NUM: n } } });
const txt = (s: string) => ({ shadow: { type: 'text', fields: { TEXT: s } } });
const ent = () => ({ shadow: { type: 'ha_entity', fields: { ENTITY: '' } } });
const dur = (n: number, unit: 'seconds' | 'minutes' | 'hours') => ({
  shadow: { type: 'duration', inputs: { N: num(n) }, fields: { UNIT: unit } },
});

/** Blocks one after another in a stack. */
function stack(...blocks: State[]): { block: State } {
  const [first, ...rest] = blocks;
  return { block: rest.length ? { ...first, next: stack(...rest) } : first };
}

const when = (trigger: State, ...actions: State[]) => ({
  blocks: {
    languageVersion: 0,
    blocks: [{ type: 'ha_when', x: 40, y: 40, inputs: { TRIGGERS: stack(trigger), DO: stack(...actions) } }],
  },
});

export interface Starter {
  key: string;
  name: string;
  /** What it does, for the card. */
  summary: string;
  mode: RunMode;
  workspace: object;
}

export const STARTERS: Starter[] = [
  {
    key: 'motion-light',
    name: 'Motion light',
    summary: 'Turns a light on when there is motion, and off 2 minutes after the last motion.',
    mode: 'single',
    workspace: when(
      { type: 'ha_trigger_becomes', fields: { ENTITY: '', STATE: 'on' }, inputs: { FOR: dur(0, 'seconds') } },
      // Running it again restarts the countdown, so every motion keeps the light on longer.
      { type: 'device_on_for', inputs: { TARGET: ent(), SECONDS: dur(2, 'minutes') } },
    ),
  },
  {
    key: 'door-reminder',
    name: 'Door left open',
    summary: 'Sends a notification when a door has been open for 10 minutes.',
    mode: 'single',
    workspace: when(
      { type: 'ha_trigger_becomes', fields: { ENTITY: '', STATE: 'on' }, inputs: { FOR: dur(10, 'minutes') } },
      {
        type: 'notify_send',
        fields: { TO: 'persistent' },
        inputs: { MESSAGE: txt('The door has been open for 10 minutes.'), TITLE: txt('Door left open') },
      },
    ),
  },
  {
    key: 'vacation-lights',
    name: 'Vacation lights',
    summary: 'Turns lights on at sunset and off at 23:00, so the house looks lived in.',
    mode: 'single',
    workspace: when(
      { type: 'ha_trigger_sun', fields: { EVENT: 'sunset' }, inputs: { OFFSET: dur(0, 'minutes') } },
      { type: 'device_power', fields: { MODE: 'on' }, inputs: { TARGET: ent() } },
      { type: 'wait_until_time', fields: { TIME: '23:00' } },
      { type: 'device_power', fields: { MODE: 'off' }, inputs: { TARGET: ent() } },
    ),
  },
  {
    key: 'good-night',
    name: 'Good night',
    summary: 'Say “good night” to Assist (or press Run): lights off, door locked, heating down.',
    mode: 'single',
    workspace: when(
      { type: 'ha_trigger_conversation', fields: { COMMAND: 'good night' } },
      { type: 'light_power', fields: { MODE: 'off' }, inputs: { TARGET: ent(), TRANSITION: num(2) } },
      { type: 'lock_command', fields: { MODE: 'lock' }, inputs: { TARGET: ent() } },
      { type: 'climate_set_temperature', inputs: { TARGET: ent(), TEMPERATURE: num(18) } },
    ),
  },
];

export const starter = (key: string) => STARTERS.find((s) => s.key === key) ?? null;
