//! One-block helpers for common devices. Targets accept an entity, a list, or an area/floor/
//! label/device block, like Home Assistant's own actions.

import * as Blockly from 'blockly/core';

type Json = Record<string, unknown>;
const value = (name: string, check?: string) => ({ type: 'input_value', name, ...(check ? { check } : {}) });
const dropdown = (name: string, options: [string, string][]) => ({ type: 'field_dropdown', name, options });
const entity = (name: string, value = '', domains: string[] = []) => ({ type: 'field_entity', name, text: value, domains });
const text = (name: string, value = '') => ({ type: 'field_input', name, text: value });

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
  val('cover_position_of', '%1 of %2', [dropdown('WHAT', [['position %', 'position'], ['tilt %', 'tilt']]), entity('ENTITY', '', ['cover'])], 'Number', 'How far a cover is open (0–100 %).'),
  act('climate_set_temperature', 'set %1 to %2 °', [value('TARGET'), value('TEMPERATURE', 'Number')], 'Sets the target temperature of a thermostat.'),
  act(
    'climate_set_mode',
    'set %1 to %2',
    [value('TARGET'), dropdown('MODE', [['heat', 'heat'], ['cool', 'cool'], ['heat/cool', 'heat_cool'], ['auto', 'auto'], ['dry', 'dry'], ['fan only', 'fan_only'], ['off', 'off']])],
    'Changes what a thermostat or air conditioner does.',
  ),
  act('climate_set_preset', 'set preset of %1 to %2', [value('TARGET'), value('PRESET', 'String')], 'Sets a thermostat preset such as eco, away or comfort (the names depend on the device).'),
  val(
    'climate_temperature_of',
    '%1 temperature of %2',
    [dropdown('WHICH', [['current', 'current'], ['target', 'target']]), entity('ENTITY', '', ['climate'])],
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
  act(
    'media_say',
    'say %1 on %2 with %3',
    [value('MESSAGE'), value('TARGET'), entity('TTS', '', ['tts'])],
    'Speaks text on speakers using a text-to-speech service (choose the tts entity, e.g. tts.google_translate_en_com).',
  ),
  val('media_volume_of', 'volume of %1', [entity('ENTITY', '', ['media_player'])], 'Number', 'The volume of a media player (0–100 %).'),
  val('media_title_of', 'what %1 is playing', [entity('ENTITY', '', ['media_player'])], 'String', 'The title of what is playing.'),
  act('fan_set_speed', 'set speed of %1 to %2 %', [value('TARGET'), value('SPEED', 'Number')], 'Sets a fan speed (0–100 %).'),
  val('fan_speed_of', 'speed of %1', [entity('ENTITY', '', ['fan'])], 'Number', 'A fan speed in percent.'),
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
    'set %1 to %2',
    [entity('ENTITY', '', ['input_number', 'input_text', 'input_select', 'input_boolean', 'input_datetime', 'counter', 'number', 'select', 'text']), value('VALUE')],
    'Sets a helper (number, text, dropdown, toggle, date/time or counter) to a value.',
  ),
  act(
    'notify_send',
    'notify %1 %2 title %3',
    [text('TO', 'persistent'), value('MESSAGE'), value('TITLE')],
    'Sends a notification. "persistent" shows it in Home Assistant; otherwise use a notify service such as mobile_app_your_phone.',
  ),
];

let registered = false;
export function registerHelperBlocks() {
  if (registered) return;
  registered = true;
  Blockly.common.defineBlocksWithJsonArray(HELPER_BLOCKS);
}
