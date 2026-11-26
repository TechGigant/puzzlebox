import * as Blockly from 'blockly/core';
import { registerFieldMultilineInput } from '@blockly/field-multilineinput';
import { registerFieldColour } from '@blockly/field-colour';
import { registerFieldEntity } from './field_entity';
import { registerConditionOptions } from './conditions';

type Json = Record<string, unknown>;

const text = (name: string, value = '') => ({ type: 'field_input', name, text: value });
const entity = (name: string, value = '', domains: string[] = []) => ({ type: 'field_entity', name, text: value, domains });
const multiline = (name: string, value = '') => ({ type: 'field_multilinetext', name, text: value, spellcheck: false });
const number = (name: string, value = 0, extra: Json = {}) => ({ type: 'field_number', name, value, ...extra });
const dropdown = (name: string, options: [string, string][]) => ({ type: 'field_dropdown', name, options });
const value = (name: string, check?: string | string[]) => ({ type: 'input_value', name, ...(check ? { check } : {}) });
const statements = (name: string, check?: string) => ({ type: 'input_statement', name, ...(check ? { check } : {}) });
const variable = (name: string, v = 'item') => ({ type: 'field_variable', name, variable: v });
const checkbox = (name: string, checked = true) => ({ type: 'field_checkbox', name, checked });
const stateValue = (name: string, value = '') => ({ type: 'field_state_value', name, text: value });
const time = (name: string, value = '') => ({ type: 'field_time', name, text: value });
/** Colours are "#rrggbb" text, so they also fit text sockets. */
const COLOUR = ['Colour', 'String'];
const dummy = { type: 'input_dummy' };

/** Short description of a trigger block for the "triggered by" dropdown. */
function describeTrigger(b: Blockly.Block): string {
  const t = b.toString(42).replace(/\s+/g, ' ').trim();
  return t || b.type;
}

/**
 * Dropdown of the workspace's trigger blocks (value = block id). Saved ids stay valid while
 * the workspace is still loading, and a deleted trigger shows as missing instead of vanishing.
 */
class FieldTriggerRef extends Blockly.FieldDropdown {
  private kept: string | null = null;

  constructor() {
    super(function (this: Blockly.FieldDropdown) {
      return (this as FieldTriggerRef).menu();
    });
  }

  static fromJson() {
    return new FieldTriggerRef();
  }

  private menu(): [string, string][] {
    const ws = this.getSourceBlock()?.workspace;
    const triggers = (ws?.getAllBlocks(true) ?? []).filter(
      (b) => !b.isShadow() && b.previousConnection?.getCheck()?.includes('Trigger'),
    );
    const opts = triggers.map((b) => [describeTrigger(b), b.id] as [string, string]);
    if (this.kept && !opts.some(([, id]) => id === this.kept)) {
      opts.push([ws?.getBlockById(this.kept) ? describeTrigger(ws.getBlockById(this.kept)!) : '(missing trigger)', this.kept]);
    }
    return opts.length ? opts : [['(add a trigger first)', '']];
  }

  protected override doClassValidation_(v?: string): string | null {
    if (typeof v === 'string') this.kept = v;
    return super.doClassValidation_(v);
  }
}

const trigger = (type: string, message0: string, args0: Json[], tooltip: string): Json => ({
  type,
  // Triggers have the same shape as actions in Blockly; the bolt tells them apart.
  message0: `⚡ ${message0}`,
  args0,
  previousStatement: 'Trigger',
  nextStatement: 'Trigger',
  style: 'trigger_blocks',
  tooltip,
});

const action = (type: string, message0: string, args0: Json[], tooltip: string, style = 'action_blocks'): Json => ({
  type,
  message0,
  args0,
  previousStatement: null,
  nextStatement: null,
  style,
  tooltip,
  inputsInline: false,
});

/** An action whose sockets sit on one line, Scratch-style ("set [x] to [y] %"). */
const inline = (type: string, message0: string, args0: Json[], tooltip: string, style = 'action_blocks'): Json => ({
  ...action(type, message0, args0, tooltip, style),
  inputsInline: true,
});

const expr = (type: string, message0: string, args0: Json[], output: string | string[] | null, style: string, tooltip: string): Json => ({
  type,
  message0,
  args0,
  output,
  style,
  tooltip,
  inputsInline: true,
});

export const CORE_BLOCKS: Json[] = [
  // --- entry points -----------------------------------------------------------
  {
    type: 'ha_when',
    message0: 'when %1 %2 do %3',
    args0: [{ type: 'input_dummy' }, statements('TRIGGERS', 'Trigger'), statements('DO')],
    style: 'entry_blocks',
    tooltip: 'Runs the blocks inside whenever any of the triggers fires.',
  },
  {
    type: 'ha_on_run',
    message0: 'when run manually %1 do %2',
    args0: [{ type: 'input_dummy' }, statements('DO')],
    style: 'entry_blocks',
    tooltip: 'Runs only when started with the Run button or the puzzlebox.run_program action.',
  },
  {
    type: 'ha_on_start',
    message0: 'when program starts %1 do %2',
    args0: [{ type: 'input_dummy' }, statements('DO')],
    style: 'entry_blocks',
    tooltip: 'Runs once when the program is enabled, saved or the app starts.',
  },

  // --- triggers -------------------------------------------------------------
  trigger(
    'ha_trigger_state',
    'state of %1 changes from %2 to %3 for %4',
    [entity('ENTITY'), stateValue('FROM'), stateValue('TO', 'on'), value('FOR', 'Number')],
    'Fires when an entity changes state (not when only its details change). Leave from/to empty to match any state; "for" makes it wait until the state has lasted that long.',
  ),
  trigger(
    'ha_trigger_numeric',
    '%1 is above %2 and below %3',
    [entity('ENTITY'), text('ABOVE', '25'), text('BELOW')],
    'Fires when the value moves into this range. Leave one side empty for just "above" or just "below". A side can also be a number helper (input_number.…).',
  ),
  trigger('ha_trigger_time', 'it is %1', [time('AT', '07:00')], 'Fires every day at this time (like 07:30), or at the time in a date/time helper (input_datetime.…).'),
  trigger(
    'ha_trigger_sun',
    'at %1 shifted by %2',
    [dropdown('EVENT', [['sunset', 'sunset'], ['sunrise', 'sunrise']]), value('OFFSET', 'Number')],
    'Fires at sunrise or sunset. A negative shift fires earlier (e.g. -30 minutes = half an hour before sunset).',
  ),
  trigger('ha_trigger_event', 'event %1 fires', [text('EVENT_TYPE', 'my_event')], 'Fires when a Home Assistant event is fired.'),
  trigger('ha_trigger_ha_start', 'Home Assistant starts', [], 'Fires when Home Assistant has started.'),
  trigger(
    'ha_trigger_every',
    'every %1 %2',
    [number('N', 5, { min: 1, precision: 1 }), dropdown('UNIT', [['minutes', 'minutes'], ['hours', 'hours'], ['seconds', 'seconds']])],
    'Fires repeatedly, e.g. every 5 minutes (at :00, :05, :10…).',
  ),
  trigger('engine_trigger_cron', 'schedule (cron) %1', [text('CRON', '0 */5 * * * *')], 'Cron schedule in local time. 5 or 6 fields; the optional first field is seconds.'),
  trigger(
    'engine_trigger_webhook',
    'webhook %1 method %2',
    [text('WEBHOOK_ID', 'my-hook'), dropdown('METHOD', [['POST', 'POST'], ['GET', 'GET'], ['PUT', 'PUT'], ['any', 'ANY']])],
    'Fires on requests to /api/webhook/<id> (on your Home Assistant URL when the Puzzlebox integration is installed). The body and query are available as trigger data.',
  ),
  trigger('engine_trigger_mqtt', 'MQTT message on %1', [text('TOPIC', 'home/+/state')], 'Fires on MQTT messages (wildcards allowed).'),
  trigger('ha_trigger_raw', 'HA trigger (YAML) %1', [multiline('CONFIG', 'trigger: state\nentity_id: sun.sun')], 'Any native Home Assistant trigger, written as YAML.'),

  // --- actions --------------------------------------------------------------
  action(
    'ha_call_service',
    'perform action %1 %2 on %3 with data %4',
    [text('SERVICE', 'light.turn_on'), { type: 'input_dummy' }, value('TARGET'), value('DATA', ['Dict', 'Object'])],
    'Calls a Home Assistant action. Target: entity id(s), a list, or a target dictionary.',
  ),
  action('ha_wait', 'wait %1', [value('SECONDS', 'Number')], 'Pauses this run for a while (a number on its own means seconds).', 'flow_blocks'),
  action(
    'ha_wait_for_trigger',
    'wait for %1 %2 at most %3',
    [{ type: 'input_dummy' }, statements('TRIGGERS', 'Trigger'), value('TIMEOUT', 'Number')],
    'Waits until one of the triggers fires, or the time runs out. Afterwards, "the wait timed out" and "what ended the wait" tell you which.',
    'flow_blocks',
  ),
  action(
    'ha_log',
    'log %1 %2',
    [dropdown('LEVEL', [['info', 'info'], ['debug', 'debug'], ['warning', 'warning'], ['error', 'error']]), value('MESSAGE')],
    'Writes to the program log.',
  ),
  action('ha_fire_event', 'fire event %1 with data %2', [value('EVENT_TYPE', 'String'), value('DATA')], 'Fires a Home Assistant event.'),
  action('ha_action_raw', 'HA action (YAML) %1', [multiline('CONFIG', 'action: notify.notify\ndata:\n  message: Hello')], 'Any native Home Assistant action sequence, written as YAML.'),

  // --- state & conditions ---------------------------------------------------
  expr('ha_entity', '%1', [entity('ENTITY')], 'String', 'condition_blocks', 'An entity. Several can be separated with commas.'),
  expr('ha_area', 'area %1', [{ type: 'field_registry', name: 'ID', text: '', kind: 'area' }], null, 'condition_blocks', 'An area, usable as an action target ("turn off the lights in the kitchen"). Several can be separated with commas.'),
  expr('ha_floor', 'floor %1', [{ type: 'field_registry', name: 'ID', text: '', kind: 'floor' }], null, 'condition_blocks', 'A floor, usable as an action target.'),
  expr('ha_label', 'label %1', [{ type: 'field_registry', name: 'ID', text: '', kind: 'label' }], null, 'condition_blocks', 'A label, usable as an action target.'),
  expr('ha_device', 'device %1', [{ type: 'field_registry', name: 'ID', text: '', kind: 'device' }], null, 'condition_blocks', 'A device, usable as an action target.'),
  expr('ha_state', 'state of %1', [value('ENTITY')], 'String', 'condition_blocks', 'What an entity is right now ("on", "21.5", "home"…), as text.'),
  expr('ha_attribute', '%1 of %2', [{ type: 'field_attribute', name: 'ATTRIBUTE', text: 'friendly_name' }, value('ENTITY')], null, 'condition_blocks', 'A detail (attribute) of an entity, e.g. a light\'s brightness (0–255) or a thermostat\'s target temperature.'),
  {
    ...expr(
      'ha_is_state',
      '%1 is %2',
      [value('ENTITY'), stateValue('STATE', 'on')],
      'Boolean',
      'condition_blocks',
      'True if the entity is in this state. Several states can be separated with commas (any of them counts). Use ＋ to compare a detail (attribute) instead, or to need just one of several entities.',
    ),
    mutator: 'pb_condition_options',
  },
  expr('ha_condition_raw', 'HA condition (YAML) %1', [multiline('CONFIG', 'condition: state\nentity_id: sun.sun\nstate: below_horizon')], 'Boolean', 'condition_blocks', 'Any native Home Assistant condition, written as YAML.'),
  expr('ha_template', 'template %1', [multiline('TEMPLATE', "{{ states('sun.sun') }}")], null, 'condition_blocks', 'Rendered by Home Assistant. Program variables and "trigger" are available.'),
  expr('ha_trigger_data', 'trigger detail %1', [text('PATH', 'to_state.state')], null, 'trigger_blocks', 'Any detail of what started this run, by its path (e.g. to_state.attributes.brightness).'),


  // --- timers, waiting & rate limits -----------------------------------------
  action(
    'timer_start',
    'start timer %1 for %2 if already counting %3 %4 then do %5',
    [text('NAME', 'lights off'), value('SECONDS', 'Number'), dropdown('MODE', [['start over', 'restart'], ['keep going', 'keep']]), dummy, statements('DO')],
    'Runs the blocks inside when the time is up, without holding up the rest. Starting it again starts the countdown over, so "turn off 5 minutes after the last motion" needs nothing else.',
    'flow_blocks',
  ),
  action('timer_cancel', 'cancel timer %1', [text('NAME', 'lights off')], 'Stops a running timer before it fires.', 'flow_blocks'),
  expr('timer_running', 'timer %1 is running', [text('NAME', 'lights off')], 'Boolean', 'flow_blocks', 'True while the timer is counting down.'),
  action(
    'wait_until',
    'wait until %1 stays true for %2 at most %3',
    [value('COND', 'Boolean'), value('HOLD', 'Number'), value('TIMEOUT', 'Number')],
    'Waits until the condition is true (and has stayed true for a while, if set), or the time runs out. Afterwards, "the wait timed out" tells you which.',
    'flow_blocks',
  ),
  expr(
    'throttle',
    'at most once every %1',
    [value('SECONDS', 'Number')],
    'Boolean',
    'flow_blocks',
    'True the first time, then false until the seconds have passed. Put it in an "if" to stop notifications from repeating. Each block keeps its own limit.',
  ),
  expr('duration', '%1 %2', [value('N', 'Number'), dropdown('UNIT', [['seconds', 'seconds'], ['minutes', 'minutes'], ['hours', 'hours'], ['days', 'days']])], 'Number', 'time_blocks', 'A duration in seconds.'),

  // --- more state & conditions ---------------------------------------------------
  {
    ...expr(
      'ha_state_for',
      '%1 has been %2 for %3',
      [value('ENTITY'), stateValue('STATE', 'off'), value('SECONDS', 'Number')],
      'Boolean',
      'condition_blocks',
      'True if the entity has been in the state for at least the given time. Several states can be separated with commas.',
    ),
    mutator: 'pb_condition_options',
  },
  expr('ha_has_value', '%1 has a value', [value('ENTITY')], 'Boolean', 'condition_blocks', 'False while the entity is unknown or unavailable (or doesn\'t exist).'),
  expr('ha_number_state', '%1 as a number (or %2 if unknown)', [value('ENTITY'), value('DEFAULT', 'Number')], 'Number', 'condition_blocks', 'The state as a number, for sensors. When the sensor has no number right now, you get the second value instead.'),
  expr('ha_friendly_name', 'name of %1', [value('ENTITY')], 'String', 'condition_blocks', 'The name of an entity as shown in Home Assistant.'),
  expr(
    'ha_seconds_since',
    'seconds since %1 %2',
    [value('ENTITY'), dropdown('FIELD', [['changed', 'last_changed'], ['updated', 'last_updated'], ['reported', 'last_reported']])],
    'Number',
    'condition_blocks',
    'How long ago the entity changed state (or reported any update).',
  ),
  expr(
    'ha_state_check',
    '%1 of %2 %3 %4',
    [
      dropdown('MODE', [['any', 'any'], ['all', 'all'], ['none', 'none']]),
      value('LIST'),
      dropdown('OP', [['are', 'is'], ['are not', 'is_not']]),
      text('STATE', 'on'),
    ],
    'Boolean',
    'condition_blocks',
    'Checks several entities at once: a list, an area, a floor, a label or a device.',
  ),
  expr(
    'ha_state_count',
    'how many of %1 %2 %3',
    [value('LIST'), dropdown('OP', [['are', 'is'], ['are not', 'is_not']]), text('STATE', 'on')],
    'Number',
    'condition_blocks',
    'Counts entities in a state: a list, an area, a floor, a label or a device.',
  ),
  expr(
    'ha_states_aggregate',
    '%1 of states of %2',
    [dropdown('OP', [['average', 'average'], ['minimum', 'min'], ['maximum', 'max'], ['sum', 'sum'], ['median', 'median']]), value('LIST')],
    'Number',
    'condition_blocks',
    'Combines the numeric states of several entities, skipping unavailable ones.',
  ),
  expr(
    'time_is_between',
    'time is between %1 and %2 %3 on Mon %4 Tue %5 Wed %6 Thu %7 Fri %8 Sat %9 Sun %10',
    [time('AFTER', '22:00'), time('BEFORE', '06:00'), dummy, checkbox('MON'), checkbox('TUE'), checkbox('WED'), checkbox('THU'), checkbox('FRI'), checkbox('SAT'), checkbox('SUN')],
    'Boolean',
    'condition_blocks',
    'True between the two times (it can span midnight) on the ticked days. Leave a time empty for no limit on that side.',
  ),
  expr(
    'ha_history',
    '%1 of %2 over the last %3 %4',
    [
      dropdown('OP', [['average', 'average'], ['minimum', 'min'], ['maximum', 'max'], ['number of changes', 'changes'], ['seconds in state', 'time_in_state']]),
      value('ENTITY'),
      value('SECONDS', 'Number'),
      stateValue('STATE'),
    ],
    'Number',
    'condition_blocks',
    'From Home Assistant\'s history. "seconds in state" needs the state in the last box. The average is weighted by time.',
  ),
  expr('ha_state_ago', 'state of %1 %2 ago', [value('ENTITY'), value('SECONDS', 'Number')], 'String', 'condition_blocks', 'What the entity was a while ago, from Home Assistant\'s history.'),
  expr(
    'ha_entities_in',
    '%1 in %2 %3',
    [
      dropdown('DOMAIN', [['all entities', '*'], ['lights', 'light'], ['switches', 'switch'], ['sensors', 'sensor'], ['binary sensors', 'binary_sensor'], ['covers', 'cover'], ['thermostats', 'climate'], ['media players', 'media_player'], ['fans', 'fan'], ['locks', 'lock'], ['scenes', 'scene'], ['people', 'person']]),
      dropdown('KIND', [['area', 'area'], ['floor', 'floor'], ['label', 'label'], ['device', 'device'], ['integration', 'integration']]),
      { type: 'field_registry', name: 'NAME', text: '', kind: 'area' },
    ],
    'Array',
    'condition_blocks',
    'The entities in an area, floor, label, device or integration, as a list.',
  ),

  // --- scenes --------------------------------------------------------------------
  expr('ha_scene_last', 'last activated scene of %1', [value('SCENES')], 'String', 'condition_blocks', 'The scene from the list that was activated most recently (empty if none was).'),
  action(
    'ha_scene_next',
    'activate %1 scene of %2',
    [dropdown('DIRECTION', [['next', 'next'], ['previous', 'previous']]), value('SCENES')],
    'Activates the scene after (or before) the last activated one in the list, wrapping around. Good for one button that cycles through scenes.',
  ),

  // --- trigger info ----------------------------------------------------------------
  expr('triggered_by', 'triggered by %1', [{ type: 'field_trigger_ref', name: 'TRIGGER' }], 'Boolean', 'trigger_blocks', 'True if this run was started by the chosen trigger.'),
  expr(
    'trigger_info',
    'trigger %1 %2',
    [
      dropdown('WHAT', [['entity', 'entity'], ['old state', 'from'], ['new state', 'to'], ['old detail', 'from_attr'], ['new detail', 'to_attr']]),
      { ...text('ATTR', ''), name: 'ATTR' },
    ],
    null,
    'trigger_blocks',
    'Details of what started this run. For details (attributes), put their name in the box.',
  ),

  // --- values ----------------------------------------------------------------------
  expr('yaml_value', 'value %1', [multiline('CONFIG', '')], null, 'dict_blocks', 'Any value, written as YAML or JSON (lists, dictionaries…). Empty means "not set".'),


  // --- lights & colours ----------------------------------------------------------
  inline(
    'light_set_colour',
    'set %1 to colour %2 at %3 % brightness, fade %4 seconds',
    [value('TARGET'), value('COLOUR'), value('BRIGHTNESS', 'Number'), value('TRANSITION', 'Number')],
    'Turns lights on in a colour (a colour block, "#rrggbb" or a colour name). Leave brightness or fade empty to keep them as they are.',
    'colour_blocks',
  ),
  inline(
    'light_set_white',
    'set %1 to white %2 K at %3 % brightness, fade %4 seconds',
    [value('TARGET'), value('KELVIN', 'Number'), value('BRIGHTNESS', 'Number'), value('TRANSITION', 'Number')],
    'Turns lights on in white: about 2200 K is candle-warm, 2700 K cosy, 4000 K neutral and 6500 K daylight.',
    'colour_blocks',
  ),
  inline(
    'light_set_brightness',
    'set brightness of %1 to %2 %, fade %3 seconds',
    [value('TARGET'), value('BRIGHTNESS', 'Number'), value('TRANSITION', 'Number')],
    'Turns lights on at a brightness, keeping their colour.',
    'colour_blocks',
  ),
  inline(
    'light_step',
    '%1 %2 by %3 %, fade %4 seconds',
    [dropdown('DIRECTION', [['brighten', 'brighten'], ['dim', 'dim']]), value('TARGET'), value('STEP', 'Number'), value('TRANSITION', 'Number')],
    'Makes lights brighter or dimmer by a step, e.g. for dimmer buttons.',
    'colour_blocks',
  ),
  inline(
    'light_power',
    '%1 %2 fade %3 seconds',
    [dropdown('MODE', [['turn on', 'on'], ['turn off', 'off'], ['toggle', 'toggle']]), value('TARGET'), value('TRANSITION', 'Number')],
    'Turns lights on, off or toggles them, optionally fading.',
    'colour_blocks',
  ),
  expr('light_colour', 'colour of %1', [value('ENTITY')], COLOUR, 'colour_blocks', 'The current colour of a light, or empty when it is off (use "or if empty" for a fallback).'),
  expr('light_brightness', 'brightness of %1 in %', [value('ENTITY')], 'Number', 'colour_blocks', 'Brightness of a light in percent (0 when off).'),
  expr('colour_picker', '%1', [{ type: 'field_colour', name: 'COLOUR', colour: '#ffcc66' }], COLOUR, 'colour_blocks', 'A colour.'),
  expr(
    'colour_rgb255',
    'colour red %1 green %2 blue %3',
    [value('RED', 'Number'), value('GREEN', 'Number'), value('BLUE', 'Number')],
    COLOUR,
    'colour_blocks',
    'A colour from red, green and blue (0–255).',
  ),
  expr(
    'colour_hsv',
    'colour hue %1 saturation % %2 brightness % %3',
    [value('HUE', 'Number'), value('SATURATION', 'Number'), value('VALUE', 'Number')],
    COLOUR,
    'colour_blocks',
    'A colour from a hue (0–360: 0 red, 120 green, 240 blue), saturation and brightness.',
  ),
  expr('colour_mix', 'mix %1 with %2 by % %3', [value('A'), value('B'), value('RATIO', 'Number')], COLOUR, 'colour_blocks', 'Blends two colours: 0 % is the first, 100 % the second.'),
  expr('colour_random', 'random colour', [], COLOUR, 'colour_blocks', 'A random bright colour.'),
  expr(
    'colour_part',
    '%1 of colour %2',
    [dropdown('PART', [['red', 'red'], ['green', 'green'], ['blue', 'blue'], ['hue', 'hue'], ['saturation', 'saturation'], ['brightness', 'value']]), value('COLOUR')],
    'Number',
    'colour_blocks',
    'A part of a colour.',
  ),

  // --- switch / organising ------------------------------------------------------
  {
    type: 'control_switch',
    message0: 'switch on %1 %2 %3',
    args0: [value('VALUE'), dummy, statements('CASES', 'Case')],
    previousStatement: null,
    nextStatement: null,
    style: 'flow_blocks',
    tooltip: 'Runs the first case whose value matches (a list matches any of its values), or "otherwise" if none does.',
  },
  {
    type: 'control_case',
    message0: 'case %1 %2 %3',
    args0: [value('VALUE'), dummy, statements('DO')],
    previousStatement: 'Case',
    nextStatement: 'Case',
    style: 'flow_blocks',
    tooltip: 'Runs when the switch value equals this (or is in this list).',
  },
  {
    type: 'control_case_default',
    message0: 'otherwise %1 %2',
    args0: [dummy, statements('DO')],
    previousStatement: 'Case',
    nextStatement: 'Case',
    style: 'flow_blocks',
    tooltip: 'Runs when no case matched.',
  },
  action('control_section', '— %1 —', [text('TEXT', 'section')], 'A label to organise long stacks. It does nothing when the program runs.', 'section_blocks'),

  // --- more logic, numbers, text and lists -------------------------------------
  expr('logic_or_default', '%1 or if empty %2', [value('A'), value('B')], null, 'logic_blocks', 'The first value, or the second one if the first is empty, unknown or unavailable.'),
  expr('math_between', '%1 is between %2 and %3', [value('VALUE', 'Number'), value('LOW', 'Number'), value('HIGH', 'Number')], 'Boolean', 'math_blocks', 'True if the number is between the two (inclusive).'),
  expr(
    'text_split_at',
    'text %1 %2 %3',
    [dropdown('WHERE', [['before', 'before'], ['after', 'after']]), value('SEP', 'String'), value('TEXT')],
    'String',
    'text_blocks',
    'The part of the text before or after the first occurrence of the separator.',
  ),
  {
    ...expr(
      'text_regex',
      '%1 regex %2 in %3 %4',
      [
        dropdown('OP', [['first match of', 'match'], ['all matches of', 'all'], ['replace', 'replace']]),
        text('PATTERN', '(\\d+)'),
        value('TEXT'),
        value('WITH'),
      ],
      null,
      'text_blocks',
      'Regular expressions. "first match" returns the first group if there is one. "replace" puts the last input in place of every match ($1 for groups).',
    ),
    inputsInline: true,
  },
  expr('lists_filter', 'items of %1 where %2', [value('LIST'), value('COND', 'Boolean')], 'Array', 'list_blocks', 'Keeps the items for which the condition is true. Use "current item" inside the condition.'),
  expr('lists_map', 'each item of %1 as %2', [value('LIST'), value('EXPR')], 'Array', 'list_blocks', 'Turns every item into a new value. Use "current item" inside.'),
  expr('lists_item', 'current item', [], null, 'list_blocks', 'The item being looked at inside "items of … where" or "each item of … as".'),

  // --- flow -----------------------------------------------------------------
  {
    type: 'control_try',
    message0: 'try %1 %2 on error %3 %4',
    args0: [{ type: 'input_dummy' }, statements('TRY'), variable('ERROR', 'error'), statements('CATCH')],
    previousStatement: null,
    nextStatement: null,
    style: 'flow_blocks',
    tooltip: 'If anything inside "try" fails, runs "on error" with the message in the variable.',
  },
  action('control_throw', 'fail with %1', [value('MESSAGE')], 'Stops with an error (catchable by try).', 'flow_blocks'),
  { ...action('control_stop', 'stop this run', [], 'Ends this run successfully.', 'flow_blocks'), nextStatement: undefined },
  {
    type: 'control_parallel',
    message0: 'run in parallel %1 %2',
    args0: [{ type: 'input_dummy' }, statements('BRANCHES', 'Branch')],
    previousStatement: null,
    nextStatement: null,
    style: 'flow_blocks',
    tooltip: 'Runs every branch at the same time and continues when all are done.',
  },
  {
    type: 'control_branch',
    message0: 'branch %1',
    args0: [statements('DO')],
    previousStatement: 'Branch',
    nextStatement: 'Branch',
    style: 'flow_blocks',
    tooltip: 'One branch of a parallel block.',
  },

  // --- dictionaries ---------------------------------------------------------
  {
    type: 'dict_create',
    message0: 'dictionary %1 %2',
    args0: [{ type: 'input_dummy' }, statements('ITEMS', 'Pair')],
    output: 'Dict',
    style: 'dict_blocks',
    tooltip: 'Builds a dictionary (e.g. action data).',
  },
  {
    type: 'dict_pair',
    message0: '%1 : %2',
    args0: [text('KEY', 'brightness'), value('VALUE')],
    previousStatement: 'Pair',
    nextStatement: 'Pair',
    style: 'dict_blocks',
    tooltip: 'A key and its value.',
  },
  expr('dict_get', 'get %1 from %2', [value('KEY'), value('DICT')], null, 'dict_blocks', 'The value stored under a key, or nothing if the key is missing.'),
  expr('dict_has', '%1 has key %2', [value('DICT'), value('KEY')], 'Boolean', 'dict_blocks', 'True if the key exists.'),
  expr('dict_keys', 'keys of %1', [value('DICT')], 'Array', 'dict_blocks', 'List of keys.'),
  expr('dict_values', 'values of %1', [value('DICT')], 'Array', 'dict_blocks', 'List of values.'),
  action('dict_set', 'in %1 set key %2 to %3', [variable('VAR', 'data'), value('KEY'), value('VALUE')], 'Sets a key on a dictionary variable.', 'dict_blocks'),
  expr('json_parse', 'parse JSON %1', [value('VALUE', 'String')], null, 'dict_blocks', 'Text → value.'),
  expr('json_stringify', 'to JSON %1', [value('VALUE')], 'String', 'dict_blocks', 'Value → JSON text.'),

  // --- text & time ----------------------------------------------------------
  expr('text_to_number', 'number from %1', [value('TEXT')], 'Number', 'text_blocks', 'The number in a text, or nothing if it isn\'t one.'),
  expr('to_text', 'text of %1', [value('VALUE')], 'String', 'text_blocks', 'Converts any value to text.'),
  expr('text_contains', '%1 contains %2', [value('TEXT'), value('FIND')], 'Boolean', 'text_blocks', 'True if the text (or list) contains the value.'),
  expr('time_now', 'now', [], 'String', 'time_blocks', 'The current date and time.'),
  expr('time_today_at', 'today at %1', [time('TIME', '07:00')], 'String', 'time_blocks', 'Today\'s date at this time.'),
  expr('time_add', '%1 plus %2', [value('DATETIME'), value('SECONDS', 'Number')], 'String', 'time_blocks', 'A date and time moved forward by a duration (a negative one goes back).'),
  expr('time_seconds_between', 'seconds from %1 to %2', [value('A'), value('B')], 'Number', 'time_blocks', 'Seconds from one date and time to another (negative if the second is earlier).'),
  expr(
    'format_duration',
    'format %1 s as %2',
    [value('SECONDS', 'Number'), dropdown('STYLE', [['words (1 h 5 min)', 'words'], ['clock (1:05:00)', 'clock']])],
    'String',
    'time_blocks',
    'Turns seconds into readable text.',
  ),
  expr(
    'time_part',
    'current %1',
    [dropdown('PART', [['hour', 'hour'], ['minute', 'minute'], ['second', 'second'], ['weekday (1 = Mon)', 'weekday'], ['day', 'day'], ['month', 'month'], ['year', 'year'], ['unix timestamp', 'timestamp']])],
    'Number',
    'time_blocks',
    'Part of the current local time.',
  ),

  // --- network --------------------------------------------------------------
  expr(
    'http_request',
    'HTTP %1 %2 body %3 headers %4',
    [dropdown('METHOD', [['GET', 'GET'], ['POST', 'POST'], ['PUT', 'PUT'], ['PATCH', 'PATCH'], ['DELETE', 'DELETE']]), value('URL', 'String'), value('BODY'), value('HEADERS')],
    'Dict',
    'http_blocks',
    'Makes an HTTP request and returns {status, ok, headers, body}. JSON bodies are parsed.',
  ),
  action(
    'mqtt_publish',
    'MQTT publish %1 to %2 retain %3',
    [value('PAYLOAD'), value('TOPIC', 'String'), { type: 'field_checkbox', name: 'RETAIN', checked: false }],
    'Publishes an MQTT message.',
    'http_blocks',
  ),
];

let registered = false;

export function registerCoreBlocks(): void {
  if (registered) return;
  registered = true;
  registerFieldMultilineInput();
  registerFieldColour();
  registerFieldEntity();
  Blockly.fieldRegistry.register('field_trigger_ref', FieldTriggerRef);
  registerConditionOptions();
  for (const b of CORE_BLOCKS) {
    if ((b as { nextStatement?: unknown }).nextStatement === undefined) delete (b as Json).nextStatement;
  }
  Blockly.common.defineBlocksWithJsonArray(CORE_BLOCKS);
}
