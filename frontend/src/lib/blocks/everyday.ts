//! Everyday blocks: presence, remembering and restoring states, alarm panels, buttons and
//! notifications with reply buttons. The compiler turns each into the trigger, action or
//! condition Home Assistant documents for it (crates/puzzlebox-ir/src/compile_everyday.rs).

import * as Blockly from 'blockly/core';
import { FieldSuggest, type Suggestion } from './field_entity';
import { haTriggerTypes, installTriggerSettings } from './trigger_settings';

type Json = Record<string, unknown>;
const value = (name: string, check?: string) => ({ type: 'input_value', name, ...(check ? { check } : {}) });
const dropdown = (name: string, options: [string, string][]) => ({ type: 'field_dropdown', name, options });
const entity = (name: string, value = '', domains: string[] = []) => ({ type: 'field_entity', name, text: value, domains });
const text = (name: string, value = '') => ({ type: 'field_input', name, text: value });
const END_ROW = { type: 'input_end_row' };

const act = (type: string, message0: string, args0: Json[], tooltip: string, style = 'home_blocks'): Json => ({
  type,
  message0,
  args0,
  previousStatement: null,
  nextStatement: null,
  style,
  tooltip,
  inputsInline: true,
});

/** Which entities the TARGET / ENTITY picker offers for each block. */
export const EVERYDAY_DOMAINS: Record<string, string[]> = {
  alarm_arm: ['alarm_control_panel'],
  alarm_disarm: ['alarm_control_panel'],
  alarm_is: ['alarm_control_panel'],
  button_press: ['button', 'input_button'],
};

export const EVERYDAY_BLOCKS: Json[] = [
  {
    type: 'ha_trigger_occupancy',
    message0: '⚡ %1 %2 for %3',
    args0: [
      dropdown('EVENT', [['the first person arrives at', 'enter'], ['the last person leaves', 'leave']]),
      entity('ZONE', 'zone.home', ['zone']),
      value('FOR', 'Number'),
    ],
    previousStatement: 'Trigger',
    nextStatement: 'Trigger',
    style: 'trigger_blocks',
    inputsInline: true,
    tooltip:
      'Fires when a zone goes from empty to someone there (the first person arrives), or from someone there to empty (the last person leaves). Counts the people set up as persons in Home Assistant. With a time, the zone has to stay that way that long.',
  },
  act(
    'scene_remember',
    'remember the state of %1 as %2',
    [value('TARGET'), text('NAME', 'before')],
    'Takes a snapshot of these lights or other entities (on/off, brightness, colour…) so "restore" can put them back later. It is kept as the scene scene.<name> until Home Assistant restarts; remembering again under the same name replaces it.',
  ),
  act('scene_restore', 'restore %1', [{ type: 'field_remembered_name', name: 'NAME', text: 'before' }], 'Puts the entities back the way "remember the state of … as" saved them under this name.'),
  act(
    'alarm_arm',
    'arm %1 as %2 with code %3',
    [
      value('TARGET'),
      dropdown('MODE', [['away', 'away'], ['home', 'home'], ['night', 'night'], ['vacation', 'vacation'], ['custom bypass', 'custom_bypass']]),
      value('CODE'),
    ],
    "Arms an alarm panel. Leave the code empty if your alarm doesn't need one.",
  ),
  act('alarm_disarm', 'disarm %1 with code %2', [value('TARGET'), value('CODE')], "Disarms an alarm panel. Leave the code empty if your alarm doesn't need one."),
  {
    type: 'alarm_is',
    message0: '%1 is %2',
    args0: [
      value('ENTITY'),
      dropdown('STATE', [['armed (any way)', 'armed'], ['disarmed', 'disarmed'], ['triggered', 'triggered'], ['arming', 'arming'], ['pending', 'pending'], ['armed away', 'armed_away'], ['armed home', 'armed_home'], ['armed night', 'armed_night'], ['armed vacation', 'armed_vacation']]),
    ],
    output: 'Boolean',
    style: 'condition_blocks',
    inputsInline: true,
    tooltip: 'Whether an alarm panel is armed, disarmed or going off. "Armed (any way)" is true for away, home, night, vacation and custom bypass.',
  },
  act('button_press', 'press %1', [value('TARGET')], 'Presses a button entity (button.…) or a button helper (input_button.…).'),
  {
    ...act(
      'notify_ask',
      'send notification %1 titled %2 to %3 %4 with buttons %5 and wait for a reply %6 %7 %8',
      [
        value('MESSAGE'),
        value('TITLE'),
        { type: 'field_notify_target', name: 'TO', text: '' },
        END_ROW,
        text('BUTTONS', 'Yes, No'),
        END_ROW,
        dropdown('LIMIT', [['at most', 'limit'], ['with no time limit', 'none']]),
        value('TIMEOUT', 'Number'),
      ],
      'Sends a notification with buttons to a phone with the Home Assistant app, then waits until one is tapped. Separate the buttons with commas (Android shows up to 3). Afterwards "the reply" is the button that was tapped, or nothing if no one answered in time.',
      'flow_blocks',
    ),
    extensions: ['pb_time_limit'],
  },
  {
    type: 'notify_reply',
    message0: 'the reply',
    output: null,
    style: 'flow_blocks',
    tooltip: 'The button tapped on the last notification sent with "and wait for a reply", or nothing if no one answered in time.',
  },
];

/** Names used by "remember the state of … as" blocks in the workspace. */
export function rememberedNames(ws: Blockly.Workspace | null | undefined): string[] {
  if (!ws) return [];
  const names = ws
    .getBlocksByType('scene_remember', false)
    .map((b) => String(b.getFieldValue('NAME') ?? '').trim())
    .filter(Boolean);
  return [...new Set(names)].sort();
}

/** A name to restore: suggests the names this program remembers, any other name stays allowed. */
class FieldRememberedName extends FieldSuggest {
  static fromJson(options: Blockly.FieldTextInputFromJsonConfig) {
    return new FieldRememberedName(options.text ?? '');
  }

  protected override load() {
    return Promise.resolve();
  }

  protected suggestions(q: string): Suggestion[] {
    return rememberedNames(this.getSourceBlock()?.workspace)
      .filter((n) => !q || n.toLowerCase().includes(q))
      .map((n) => ({ value: n, label: n, meta: 'remembered in this program' }));
  }

  protected emptyText() {
    return 'Type the name used in "remember the state of … as"';
  }
}

const VARIABLE_HELP: Record<string, string> = {
  '': 'A variable keeps its value only during this run of the program; it is gone when the run ends.',
  Program: 'A program variable is kept between runs of this program, even when Home Assistant or Puzzlebox restarts.',
  Global: 'A global variable is shared by all programs, kept across restarts, and shown in Home Assistant as a sensor.',
};

/** Adds what kind of variable it is (and how long it lasts) to the variable blocks' help. */
function installVariableHelp() {
  for (const type of ['variables_get', 'variables_set']) {
    const def = Blockly.Blocks[type] as { init: (this: Blockly.Block) => void } | undefined;
    if (!def) continue;
    const init = def.init;
    def.init = function (this: Blockly.Block) {
      init.call(this);
      const base = this.tooltip;
      this.setTooltip(() => {
        const own = typeof base === 'function' ? base() : String(base ?? '');
        const kind = (this.getField('VAR') as Blockly.FieldVariable | null)?.getVariable()?.getType() ?? '';
        return [own, VARIABLE_HELP[kind]].filter(Boolean).join('\n');
      });
    };
  }
}

let registered = false;
export function registerEverydayBlocks() {
  if (registered) return;
  registered = true;
  Blockly.fieldRegistry.register('field_remembered_name', FieldRememberedName);
  Blockly.common.defineBlocksWithJsonArray(EVERYDAY_BLOCKS);
  installTriggerSettings(haTriggerTypes(EVERYDAY_BLOCKS));
  installVariableHelp();
}
