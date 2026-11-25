import * as Blockly from 'blockly/core';
import { registerFieldMultilineInput } from '@blockly/field-multilineinput';
import { registerFieldColour } from '@blockly/field-colour';
import { registerFieldEntity } from './field_entity';

type Json = Record<string, unknown>;

const text = (name: string, value = '') => ({ type: 'field_input', name, text: value });
const entity = (name: string, value = '', domains: string[] = []) => ({ type: 'field_entity', name, text: value, domains });
const multiline = (name: string, value = '') => ({ type: 'field_multilinetext', name, text: value, spellcheck: false });
const number = (name: string, value = 0, extra: Json = {}) => ({ type: 'field_number', name, value, ...extra });
const dropdown = (name: string, options: [string, string][]) => ({ type: 'field_dropdown', name, options });
const value = (name: string, check?: string | string[]) => ({ type: 'input_value', name, ...(check ? { check } : {}) });
const statements = (name: string, check?: string) => ({ type: 'input_statement', name, ...(check ? { check } : {}) });
const variable = (name: string, v = 'item') => ({ type: 'field_variable', name, variable: v });

const trigger = (type: string, message0: string, args0: Json[], tooltip: string): Json => ({
  type,
  message0,
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

const expr = (type: string, message0: string, args0: Json[], output: string | null, style: string, tooltip: string): Json => ({
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
    style: 'trigger_blocks',
    hat: 'cap',
    tooltip: 'Runs the blocks inside whenever any of the triggers fires.',
  },
  {
    type: 'ha_on_run',
    message0: 'when run manually %1 do %2',
    args0: [{ type: 'input_dummy' }, statements('DO')],
    style: 'trigger_blocks',
    hat: 'cap',
    tooltip: 'Runs only when started with the Run button or the puzzlebox.run_program action.',
  },
  {
    type: 'ha_on_start',
    message0: 'when program starts %1 do %2',
    args0: [{ type: 'input_dummy' }, statements('DO')],
    style: 'trigger_blocks',
    hat: 'cap',
    tooltip: 'Runs once when the program is enabled, saved or the app starts.',
  },

  // --- triggers -------------------------------------------------------------
  trigger(
    'ha_trigger_state',
    'state of %1 changes from %2 to %3 for %4 s',
    [entity('ENTITY', 'binary_sensor.motion'), text('FROM'), text('TO', 'on'), number('FOR', 0, { min: 0 })],
    'Fires when an entity changes state. Leave from/to empty to match any state.',
  ),
  trigger(
    'ha_trigger_numeric',
    '%1 goes above %2 or below %3',
    [entity('ENTITY', 'sensor.temperature'), text('ABOVE', '25'), text('BELOW')],
    'Fires when a numeric state crosses a threshold. Leave a side empty to ignore it.',
  ),
  trigger('ha_trigger_time', 'time is %1', [text('AT', '07:00:00')], 'Fires daily at a time (HH:MM:SS) or an input_datetime entity.'),
  trigger('ha_trigger_sun', '%1 with offset %2 s', [dropdown('EVENT', [['sunset', 'sunset'], ['sunrise', 'sunrise']]), number('OFFSET', 0)], 'Fires at sunrise or sunset.'),
  trigger('ha_trigger_event', 'event %1 fires', [text('EVENT_TYPE', 'my_event')], 'Fires when a Home Assistant event is fired.'),
  trigger('ha_trigger_ha_start', 'Home Assistant starts', [], 'Fires when Home Assistant has started.'),
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
  action('ha_wait', 'wait %1 seconds', [value('SECONDS', 'Number')], 'Pauses this run.'),
  action(
    'ha_wait_for_trigger',
    'wait until %1 %2 timeout %3 s',
    [{ type: 'input_dummy' }, statements('TRIGGERS', 'Trigger'), value('TIMEOUT', 'Number')],
    'Waits for one of the triggers (or the timeout). The variable "wait" holds {"trigger": ...}.',
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
  expr('ha_entity', 'entity %1', [entity('ENTITY', 'light.kitchen')], 'String', 'condition_blocks', 'An entity id.'),
  expr('ha_state', 'state of %1', [entity('ENTITY', 'sensor.temperature')], null, 'condition_blocks', 'Current state of an entity.'),
  expr('ha_attribute', '%1 of %2', [text('ATTRIBUTE', 'brightness'), entity('ENTITY', 'light.kitchen')], null, 'condition_blocks', 'An attribute of an entity.'),
  expr('ha_is_state', '%1 is %2', [entity('ENTITY', 'light.kitchen'), text('STATE', 'on')], 'Boolean', 'condition_blocks', 'True if the entity is in the given state.'),
  expr('ha_condition_raw', 'HA condition (YAML) %1', [multiline('CONFIG', 'condition: state\nentity_id: sun.sun\nstate: below_horizon')], 'Boolean', 'condition_blocks', 'Any native Home Assistant condition, written as YAML.'),
  expr('ha_template', 'template %1', [multiline('TEMPLATE', "{{ states('sun.sun') }}")], null, 'condition_blocks', 'Rendered by Home Assistant. Program variables and "trigger" are available.'),
  expr('ha_trigger_data', 'trigger %1', [text('PATH', 'to_state.state')], null, 'trigger_blocks', 'A value from what triggered this run (dotted path).'),

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
  expr('dict_get', 'get %1 from %2', [value('KEY'), value('DICT')], null, 'dict_blocks', 'Value for a key (null if missing).'),
  expr('dict_has', '%1 has key %2', [value('DICT'), value('KEY')], 'Boolean', 'dict_blocks', 'True if the key exists.'),
  expr('dict_keys', 'keys of %1', [value('DICT')], 'Array', 'dict_blocks', 'List of keys.'),
  expr('dict_values', 'values of %1', [value('DICT')], 'Array', 'dict_blocks', 'List of values.'),
  action('dict_set', 'in %1 set key %2 to %3', [variable('VAR', 'data'), value('KEY'), value('VALUE')], 'Sets a key on a dictionary variable.', 'dict_blocks'),
  expr('json_parse', 'parse JSON %1', [value('VALUE', 'String')], null, 'dict_blocks', 'Text → value.'),
  expr('json_stringify', 'to JSON %1', [value('VALUE')], 'String', 'dict_blocks', 'Value → JSON text.'),

  // --- text & time ----------------------------------------------------------
  expr('text_to_number', 'number from %1', [value('TEXT')], 'Number', 'text_blocks', 'Parses a number (null if not numeric).'),
  expr('to_text', 'text of %1', [value('VALUE')], 'String', 'text_blocks', 'Converts any value to text.'),
  expr('text_contains', '%1 contains %2', [value('TEXT'), value('FIND')], 'Boolean', 'text_blocks', 'True if the text (or list) contains the value.'),
  expr('time_now', 'now', [], 'String', 'time_blocks', 'Current local date/time (ISO 8601).'),
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
  for (const b of CORE_BLOCKS) {
    if ((b as { nextStatement?: unknown }).nextStatement === undefined) delete (b as Json).nextStatement;
  }
  Blockly.common.defineBlocksWithJsonArray(CORE_BLOCKS);
}
