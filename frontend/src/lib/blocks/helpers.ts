//! One-block helpers for common devices. Targets accept an entity, a list, or an area/floor/
//! label/device block, like Home Assistant's own actions.

import * as Blockly from 'blockly/core';
import { allBlockDefs } from './dynamic';
import { FieldOptions, cachedRegistry, knownEntities } from './field_entity';
import { haTriggerTypes, installTriggerSettings } from './trigger_settings';

type Json = Record<string, unknown>;
const value = (name: string, check?: string) => ({ type: 'input_value', name, ...(check ? { check } : {}) });
const dropdown = (name: string, options: [string, string][]) => ({ type: 'field_dropdown', name, options });
const entity = (name: string, value = '', domains: string[] = []) => ({ type: 'field_entity', name, text: value, domains });
const text = (name: string, value = '') => ({ type: 'field_input', name, text: value });
const stateValue = (name: string, value = '') => ({ type: 'field_state_value', name, text: value });
const time = (name: string, value = '') => ({ type: 'field_time', name, text: value });
const statements = (name: string) => ({ type: 'input_statement', name });

const act = (type: string, message0: string, args0: Json[], tooltip: string): Json => ({
  type,
  message0,
  args0,
  previousStatement: null,
  nextStatement: null,
  style: 'home_blocks',
  tooltip,
  inputsInline: true,
});
const val = (type: string, message0: string, args0: Json[], output: string | null, tooltip: string): Json => ({
  type,
  message0,
  args0,
  output,
  style: 'home_blocks',
  tooltip,
  inputsInline: true,
});

/** Which entities the ENTITY picker offers for each value block. */
export const ENTITY_DOMAINS: Record<string, string[]> = {
  light_colour: ['light'],
  light_brightness: ['light'],
  cover_position_of: ['cover'],
  climate_temperature_of: ['climate'],
  media_volume_of: ['media_player'],
  media_title_of: ['media_player'],
  fan_speed_of: ['fan'],
  person_is_home: ['person', 'device_tracker'],
};

/** Which entities the TARGET picker offers for each helper block. */
export const HELPER_DOMAINS: Record<string, string[]> = {
  cover_command: ['cover'],
  cover_set_position: ['cover'],
  climate_set_temperature: ['climate'],
  climate_set_mode: ['climate'],
  climate_set_preset: ['climate'],
  media_command: ['media_player'],
  media_set_volume: ['media_player'],
  media_play: ['media_player'],
  media_say: ['media_player'],
  fan_set_speed: ['fan'],
  lock_command: ['lock'],
  vacuum_command: ['vacuum'],
  scene_activate: ['scene'],
  script_run: ['script'],
  counter_command: ['counter'],
};

export const HELPER_BLOCKS: Json[] = [
  act('device_power', '%1 %2', [dropdown('MODE', [['turn on', 'on'], ['turn off', 'off'], ['toggle', 'toggle']]), value('TARGET')], 'Turns anything with an on/off switch on or off: switches, lights, fans, plugs, helpers…'),
  act(
    'cover_command',
    '%1 %2',
    [dropdown('MODE', [['open', 'open'], ['close', 'close'], ['stop', 'stop'], ['tilt open', 'open_tilt'], ['tilt closed', 'close_tilt']]), value('TARGET')],
    'Opens, closes or stops blinds, shutters, garage doors and other covers.',
  ),
  act(
    'cover_set_position',
    'set %1 of %2 to %3 %',
    [dropdown('WHAT', [['position', 'position'], ['tilt', 'tilt']]), value('TARGET'), value('POSITION', 'Number')],
    'Moves a cover to a position: 0 % is closed, 100 % open.',
  ),
  val('cover_position_of', '%1 of %2', [dropdown('WHAT', [['position %', 'position'], ['tilt %', 'tilt']]), value('ENTITY')], 'Number', 'How far a cover is open (0–100 %).'),
  act('climate_set_temperature', 'set target temperature of %1 to %2 °', [value('TARGET'), value('TEMPERATURE', 'Number')], 'Sets the target temperature of a thermostat.'),
  act(
    'climate_set_mode',
    'set mode of %1 to %2',
    [value('TARGET'), dropdown('MODE', [['heat', 'heat'], ['cool', 'cool'], ['heat/cool', 'heat_cool'], ['auto', 'auto'], ['dry', 'dry'], ['fan only', 'fan_only'], ['off', 'off']])],
    'Changes what a thermostat or air conditioner does.',
  ),
  act('climate_set_preset', 'set preset of %1 to %2', [value('TARGET'), value('PRESET', 'String')], 'Sets a thermostat preset such as eco, away or comfort (the names depend on the device).'),
  val(
    'climate_temperature_of',
    '%1 temperature of %2',
    [dropdown('WHICH', [['current', 'current'], ['target', 'target']]), value('ENTITY')],
    'Number',
    'The room temperature a thermostat measures, or the one it aims for.',
  ),
  act(
    'media_command',
    '%1 %2',
    [
      dropdown('MODE', [['play', 'play'], ['pause', 'pause'], ['play/pause', 'play_pause'], ['stop', 'stop'], ['next track on', 'next'], ['previous track on', 'previous'], ['mute', 'mute'], ['unmute', 'unmute'], ['turn on', 'on'], ['turn off', 'off']]),
      value('TARGET'),
    ],
    'Controls a speaker, TV or other media player.',
  ),
  act('media_set_volume', 'set volume of %1 to %2 %', [value('TARGET'), value('VOLUME', 'Number')], 'Sets the volume (0–100 %).'),
  act(
    'media_play',
    'play %1 %2 on %3',
    [dropdown('TYPE', [['music', 'music'], ['video', 'video'], ['playlist', 'playlist'], ['radio / URL', 'url'], ['channel', 'channel']]), value('MEDIA', 'String'), value('TARGET')],
    'Plays a URL, file or media ID on a media player.',
  ),
  {
    ...act(
      'media_say',
      'say %1 on %2 with %3',
      [value('MESSAGE'), value('TARGET'), entity('TTS', '', ['tts'])],
      'Speaks text on speakers using a text-to-speech service (choose the tts entity, e.g. tts.google_translate_en_com).',
    ),
    extensions: ['pb_default_tts'],
  },
  val('media_volume_of', 'volume of %1', [value('ENTITY')], 'Number', 'The volume of a media player (0–100 %).'),
  val('media_title_of', 'what %1 is playing', [value('ENTITY')], 'String', 'The title of what is playing.'),
  act('fan_set_speed', 'set speed of %1 to %2 %', [value('TARGET'), value('SPEED', 'Number')], 'Sets a fan speed (0–100 %).'),
  val('fan_speed_of', 'speed of %1', [value('ENTITY')], 'Number', 'A fan speed in percent.'),
  act('lock_command', '%1 %2', [dropdown('MODE', [['lock', 'lock'], ['unlock', 'unlock'], ['open', 'open']]), value('TARGET')], 'Locks or unlocks a lock.'),
  act(
    'vacuum_command',
    '%1 %2',
    [dropdown('MODE', [['start', 'start'], ['pause', 'pause'], ['stop', 'stop'], ['send home', 'return_to_base'], ['locate', 'locate']]), value('TARGET')],
    'Controls a robot vacuum.',
  ),
  act('scene_activate', 'activate scene %1', [value('TARGET')], 'Activates a scene.'),
  act('script_run', 'run script %1 with %2', [value('TARGET'), value('DATA')], 'Runs a Home Assistant script. The dictionary (optional) becomes its variables.'),
  act('counter_command', '%1 %2', [dropdown('MODE', [['add 1 to', 'increment'], ['take 1 from', 'decrement'], ['reset', 'reset']]), value('TARGET')], 'Changes a counter helper.'),
  act(
    'helper_set',
    'set helper %1 to %2',
    [entity('ENTITY', '', ['input_number', 'input_text', 'input_select', 'input_boolean', 'input_datetime', 'counter', 'number', 'select', 'text']), value('VALUE')],
    'Sets a helper (number, text, dropdown, toggle, date/time or counter) to a value.',
  ),
  act(
    'notify_send',
    'send notification %1 titled %2 to %3',
    [value('MESSAGE'), value('TITLE'), { type: 'field_notify_target', name: 'TO', text: 'persistent' }],
    'Sends a notification to a phone (mobile_app_…) or another notify service. "persistent" shows it in Home Assistant\'s sidebar.',
  ),
  act('device_on_for', 'turn on %1 for %2 then off', [value('TARGET'), value('SECONDS', 'Number')], 'Turns something on and, after the time, off again. Running it again restarts the countdown.'),
  {
    ...act('wait_until_time', 'wait until it is %1', [time('TIME', '07:00')], 'Waits until this time of day (or the time in a date/time helper).'),
    style: 'flow_blocks',
  },
  {
    ...act('http_send', 'send HTTP %1 to %2 with body %3', [dropdown('METHOD', [['POST', 'POST'], ['GET', 'GET'], ['PUT', 'PUT'], ['DELETE', 'DELETE']]), value('URL', 'String'), value('BODY')], 'Sends a web request and carries on (use the "HTTP" value block to read the answer).'),
    style: 'http_blocks',
  },
  // Triggers: same shape as other triggers, with the bolt.
  {
    type: 'ha_trigger_becomes',
    message0: '⚡ %1 becomes %2 for %3',
    args0: [entity('ENTITY'), stateValue('STATE', 'on'), value('FOR', 'Number')],
    previousStatement: 'Trigger',
    nextStatement: 'Trigger',
    style: 'trigger_blocks',
    inputsInline: true,
    tooltip: 'Fires when an entity changes to this state (and, if set, has stayed in it that long).',
  },
  {
    type: 'ha_trigger_zone',
    message0: '⚡ %1 %2 %3',
    args0: [entity('PERSON', '', ['person', 'device_tracker']), dropdown('EVENT', [['arrives at', 'enter'], ['leaves', 'leave']]), entity('ZONE', 'zone.home', ['zone'])],
    previousStatement: 'Trigger',
    nextStatement: 'Trigger',
    style: 'trigger_blocks',
    tooltip: 'Fires when a person arrives at or leaves a zone (Home, Work…).',
  },
  // Sensing
  { ...val('sun_is', 'the sun is %1', [dropdown('WHERE', [['up', 'up'], ['down', 'down']])], 'Boolean', 'True during the day (up) or at night (down).'), style: 'condition_blocks' },
  {
    ...val('someone_home', '%1 is home', [dropdown('WHO', [['someone', 'someone'], ['nobody', 'nobody'], ['everyone', 'everyone']])], 'Boolean', 'Looks at everyone set up as a person in Home Assistant and whether they are at home.'),
    style: 'condition_blocks',
  },
  { ...val('person_is_home', '%1 is %2', [value('ENTITY'), dropdown('WHERE', [['home', 'home'], ['away', 'away']])], 'Boolean', 'Whether a person is at home.'), style: 'condition_blocks' },
  {
    ...val(
      'weekday_is',
      'today is %1',
      [dropdown('WHEN', [['a weekday', 'weekday'], ['the weekend', 'weekend'], ['Monday', 'mon'], ['Tuesday', 'tue'], ['Wednesday', 'wed'], ['Thursday', 'thu'], ['Friday', 'fri'], ['Saturday', 'sat'], ['Sunday', 'sun']])],
      'Boolean',
      'Checks the day of the week.',
    ),
    style: 'condition_blocks',
  },
  { ...val('time_of_day', 'time of day', [], 'String', 'The current time as hours:minutes:seconds, e.g. "07:30:00".'), style: 'time_blocks' },
  { ...val('wait_timed_out', 'the wait timed out', [], 'Boolean', 'True if the last "wait until" or "wait for" ran out of time instead of finishing.'), style: 'flow_blocks' },
  { ...val('wait_entity', 'what ended the wait', [], 'String', 'The entity whose trigger ended the last "wait for" (empty if it timed out).'), style: 'flow_blocks' },
  { ...val('trigger_entity', 'trigger entity', [], 'String', 'The entity that started this run (empty for schedules and other triggers without one).'), style: 'trigger_blocks' },
  { ...val('text_regex_test', 'text %1 matches %2', [value('TEXT'), text('PATTERN', '\\d+')], 'Boolean', 'True if the text matches the regular expression.'), style: 'text_blocks' },
  { ...val('colour_hex', 'hex code of %1', [value('COLOUR')], 'String', 'A colour as "#rrggbb" text.'), style: 'colour_blocks' },
];

const slug = (s: string) =>
  s.toLowerCase().normalize('NFKD').replace(/[\u0300-\u036f]/g, '').replace(/[^a-z0-9]+/g, '_').replace(/^_|_$/g, '');

/** "mobile_app_pixel_7" → "📱 Pixel 7" (the phone's device name when it is known). */
export function notifyLabel(service: string): string {
  if (service === 'persistent') return 'Home Assistant sidebar';
  const phone = service.match(/^mobile_app_(.+)$/)?.[1];
  if (!phone) return service.replaceAll('_', ' ');
  const device = cachedRegistry()?.devices.find((d) => d.name && slug(d.name) === phone);
  return `📱 ${device?.name ?? phone.split('_').map((w) => w[0].toUpperCase() + w.slice(1)).join(' ')}`;
}

/** Notify services offered by Home Assistant (from the generated "notify" blocks). */
class FieldNotifyTarget extends FieldOptions {
  static fromJson(options: Blockly.FieldTextInputFromJsonConfig) {
    return new FieldNotifyTarget(options.text ?? 'persistent');
  }

  constructor(value: string) {
    const services = [...allBlockDefs()]
      .map((d) => d.type.match(/^ha_svc__notify__(.+)$/)?.[1])
      .filter((s): s is string => !!s && s !== 'send_message' && s !== 'persistent_notification');
    super(value, [
      { value: 'persistent', label: notifyLabel('persistent'), meta: 'persistent notification' },
      ...services.map((s) => ({ value: s, label: notifyLabel(s), meta: `notify.${s}` })),
    ]);
  }

  protected override getText_(): string | null {
    const v = String(this.getValue() ?? '').trim();
    return v ? v.split(',').map((s) => notifyLabel(s.trim())).join(', ') : null;
  }
}

/** Event groups of workspace loads: their blocks are saved ones. */
const loads = new Set<string>();

/** "say … on …" from the toolbox or quick add starts with the first voice (tts entity). */
function registerDefaultTts() {
  Blockly.Extensions.register('pb_default_tts', function (this: Blockly.Block) {
    this.setOnChange((e) => {
      if (e.type === Blockly.Events.FINISHED_LOADING && e.group) loads.add(e.group);
      // Read-only: the minimap, which mirrors the editor's blocks.
      if (e.type !== Blockly.Events.BLOCK_CREATE || this.isInFlyout || this.workspace.options.readOnly) return;
      if ((e as Blockly.Events.BlockCreate).blockId !== this.id || this.getFieldValue('TTS')) return;
      // A load announces itself after its blocks are created, in the same batch of events.
      setTimeout(() => {
        if ((e.group && loads.has(e.group)) || this.isDeadOrDying() || this.getFieldValue('TTS')) return;
        const tts = knownEntities().find((s) => s.entity_id.startsWith('tts.'));
        if (!tts) return;
        Blockly.Events.setGroup(e.group || true);
        this.setFieldValue(tts.entity_id, 'TTS');
        Blockly.Events.setGroup(false);
      });
    });
  });
}

let registered = false;
export function registerHelperBlocks() {
  if (registered) return;
  registered = true;
  Blockly.fieldRegistry.register('field_notify_target', FieldNotifyTarget);
  registerDefaultTts();
  Blockly.common.defineBlocksWithJsonArray(HELPER_BLOCKS);
  installTriggerSettings(haTriggerTypes(HELPER_BLOCKS));
}
