import * as Blockly from 'blockly/core';

type Item = Blockly.utils.toolbox.ToolboxItemInfo;

const num = (n: number) => ({ shadow: { type: 'math_number', fields: { NUM: n } } });
const txt = (s: string) => ({ shadow: { type: 'text', fields: { TEXT: s } } });
/** An empty entity picker: click it to choose. */
const ent = (e = '') => ({ shadow: { type: 'ha_entity', fields: { ENTITY: e } } });
const dur = (n: number, unit: 'seconds' | 'minutes' | 'hours' = 'seconds') => ({
  shadow: { type: 'duration', inputs: { N: num(n) }, fields: { UNIT: unit } },
});
const bool = (b = true) => ({ shadow: { type: 'logic_boolean', fields: { BOOL: b ? 'TRUE' : 'FALSE' } } });
const colour = (c: string) => ({ shadow: { type: 'colour_picker', fields: { COLOUR: c } } });
const entities = (n: number) => ({
  block: {
    type: 'lists_create_with',
    extraState: { itemCount: n },
    inputs: Object.fromEntries(Array.from({ length: n }, (_, i) => [`ADD${i}`, ent()])),
  },
});

const label = (text: string): Item => ({ kind: 'label', text }) as Item;
const sep = { kind: 'sep' } as Item;
const block = (type: string, inputs?: Record<string, unknown>, extra: Record<string, unknown> = {}): Item =>
  ({ kind: 'block', type, ...(inputs ? { inputs } : {}), ...extra }) as Item;

const cat = (name: string, style: string, contents: Item[], extra: Record<string, unknown> = {}): Item =>
  ({ kind: 'category', name, categorystyle: `${style}_category`, contents, ...extra }) as Item;

/** Categories contributed at runtime (autogen + plugins); merged in by buildToolbox. */
export interface ExtraCategory {
  name: string;
  style: string;
  blocks: Item[];
  subcategories?: ExtraCategory[];
  /** Flyout filled by a registered callback instead of `blocks`. */
  custom?: string;
}

function extraToItem(c: ExtraCategory): Item {
  if (c.custom) return cat(c.name, c.style, [], { custom: c.custom });
  const contents: Item[] = [...(c.subcategories ?? []).map(extraToItem), ...c.blocks];
  return cat(c.name, c.style, contents);
}

/**
 * Scratch-like order: events, control and sensing first, then devices and operators, data,
 * then what Home Assistant and plugins add, with technical blocks last under "Advanced".
 */
export function buildToolbox(extra: ExtraCategory[] = []): Blockly.utils.toolbox.ToolboxInfo {
  return {
    kind: 'categoryToolbox',
    contents: [
      cat('Favourites', 'favourite', [], { custom: 'PB_FAVOURITES' }),
      cat('Events', 'trigger', [
        block('ha_when'),
        block('ha_on_start'),
        block('ha_on_run'),
        label('Triggers: put them in a "when" block'),
        block('ha_trigger_becomes', { FOR: dur(0) }),
        block('ha_trigger_state', { FOR: dur(0) }),
        block('ha_trigger_numeric', { FOR: dur(0) }),
        block('ha_trigger_time', { OFFSET: dur(0, 'minutes') }),
        block('ha_trigger_every'),
        block('ha_trigger_time_pattern'),
        block('ha_trigger_sun', { OFFSET: dur(0, 'minutes') }),
        block('ha_trigger_calendar', { OFFSET: dur(0, 'minutes') }),
        block('ha_trigger_zone'),
        block('ha_trigger_tag'),
        block('ha_trigger_conversation'),
        block('ha_trigger_ha_start'),
        block('ha_trigger_event'),
        label('About this run'),
        block('trigger_entity'),
        block('trigger_info'),
        block('triggered_by'),
      ]),
      cat('Control', 'flow', [
        block('controls_if', { IF0: bool() }),
        block('controls_if', { IF0: bool() }, { extraState: { hasElse: true } }),
        block('control_switch', {
          CASES: {
            block: {
              type: 'control_case',
              inputs: { VALUE: txt('day') },
              next: { block: { type: 'control_case', inputs: { VALUE: txt('night') }, next: { block: { type: 'control_case_default' } } } },
            },
          },
        }),
        block('control_case', { VALUE: txt('value') }),
        block('control_case_default'),
        label('Wait'),
        block('ha_wait', { SECONDS: dur(5) }),
        block('wait_until', { COND: bool(), HOLD: dur(0), TIMEOUT: dur(10, 'minutes') }),
        block('ha_wait_for_trigger', { TIMEOUT: dur(10, 'minutes') }),
        block('wait_until_time'),
        block('wait_timed_out'),
        block('wait_entity'),
        label('Timers'),
        block('timer_start', { SECONDS: dur(5, 'minutes') }),
        block('timer_cancel'),
        block('timer_running'),
        block('throttle', { SECONDS: dur(10, 'minutes') }),
        label('Repeat'),
        block('controls_repeat_ext', { TIMES: num(3) }),
        block('controls_whileUntil', { BOOL: bool() }),
        block('controls_for', { FROM: num(1), TO: num(10), BY: num(1) }),
        block('controls_forEach'),
        block('loop_info'),
        block('controls_flow_statements'),
        block('control_stop'),
        label('Organise'),
        block('control_section'),
        block('ha_log', { MESSAGE: txt('Hello') }),
      ]),
      cat('Sensing', 'condition', [
        label('Is it…?'),
        block('ha_is_state', { ENTITY: ent() }),
        block('ha_state_for', { ENTITY: ent(), SECONDS: dur(5, 'minutes') }),
        block('ha_has_value', { ENTITY: ent() }),
        block('ha_numeric_state', { ENTITY: ent() }),
        block('zone_condition', { ENTITY: ent() }),
        block('sun_is'),
        block('sun_condition', { OFFSET: dur(0, 'minutes') }),
        block('someone_home'),
        block('person_is_home', { ENTITY: ent() }),
        block('weekday_is'),
        block('time_is_between'),
        label('Read a value'),
        block('ha_state', { ENTITY: ent() }),
        block('ha_number_state', { ENTITY: ent(), DEFAULT: num(0) }),
        block('ha_attribute', { ENTITY: ent() }),
        block('ha_friendly_name', { ENTITY: ent() }),
        block('ha_seconds_since', { ENTITY: ent() }),
        label('Entities, areas and more'),
        block('ha_entity'),
        block('ha_area'),
        block('ha_floor'),
        block('ha_label'),
        block('ha_device'),
        block('ha_entities_in'),
        label('Several entities'),
        block('ha_state_check', { LIST: entities(2) }),
        block('ha_state_count', { LIST: entities(2) }),
        block('ha_states_aggregate', { LIST: entities(2) }),
        label('History'),
        block('ha_history', { ENTITY: ent(), SECONDS: dur(1, 'hours') }),
        block('ha_state_ago', { ENTITY: ent(), SECONDS: dur(1, 'hours') }),
        label('Scenes'),
        block('ha_scene_last', { SCENES: entities(2) }),
      ]),
      sep,
      cat('Home', 'home', [
        label('Switches & plugs'),
        block('device_power', { TARGET: ent() }),
        block('device_on_for', { TARGET: ent(), SECONDS: dur(5, 'minutes') }),
        label('Covers'),
        block('cover_command', { TARGET: ent() }),
        block('cover_set_position', { TARGET: ent(), POSITION: num(50) }),
        block('cover_position_of', { ENTITY: ent() }),
        label('Climate'),
        block('climate_set_temperature', { TARGET: ent(), TEMPERATURE: num(21) }),
        block('climate_set_mode', { TARGET: ent() }),
        block('climate_set_preset', { TARGET: ent(), PRESET: txt('eco') }),
        block('climate_temperature_of', { ENTITY: ent() }),
        label('Media'),
        block('media_command', { TARGET: ent() }),
        block('media_set_volume', { TARGET: ent(), VOLUME: num(30) }),
        block('media_play', { TARGET: ent(), MEDIA: txt('https://') }),
        block('media_say', { MESSAGE: txt('Dinner is ready'), TARGET: ent() }),
        block('media_volume_of', { ENTITY: ent() }),
        block('media_title_of', { ENTITY: ent() }),
        label('Fans, locks & vacuums'),
        block('fan_set_speed', { TARGET: ent(), SPEED: num(50) }),
        block('fan_speed_of', { ENTITY: ent() }),
        block('lock_command', { TARGET: ent() }),
        block('vacuum_command', { TARGET: ent() }),
        label('Scenes, scripts & helpers'),
        block('scene_activate', { TARGET: ent() }),
        block('ha_scene_next', { SCENES: entities(2) }),
        block('script_run', { TARGET: ent() }),
        block('helper_set', { VALUE: txt('') }),
        block('counter_command', { TARGET: ent() }),
        label('Notifications'),
        block('notify_send', { MESSAGE: txt('Hello from Puzzlebox'), TITLE: txt('Puzzlebox') }),
      ]),
      cat('Lights & colours', 'colour', [
        block('light_set_colour', { TARGET: ent(), COLOUR: colour('#ffcc66'), BRIGHTNESS: num(80), TRANSITION: num(0) }),
        block('light_set_white', { TARGET: ent(), KELVIN: num(2700), BRIGHTNESS: num(80), TRANSITION: num(0) }),
        block('light_set_brightness', { TARGET: ent(), BRIGHTNESS: num(50), TRANSITION: num(1) }),
        block('light_step', { TARGET: ent(), STEP: num(10), TRANSITION: num(0) }),
        block('light_power', { TARGET: ent(), TRANSITION: num(0) }),
        block('light_colour', { ENTITY: ent() }),
        block('light_brightness', { ENTITY: ent() }),
        label('Colours'),
        block('colour_picker'),
        block('colour_rgb255', { RED: num(255), GREEN: num(160), BLUE: num(60) }),
        block('colour_hsv', { HUE: num(30), SATURATION: num(80), VALUE: num(100) }),
        block('colour_mix', { A: colour('#ff0000'), B: colour('#0000ff'), RATIO: num(50) }),
        block('colour_random'),
        block('colour_part', { COLOUR: colour('#ff8000') }),
        block('colour_hex', { COLOUR: colour('#ff8000') }),
      ]),
      sep,
      cat('Logic', 'logic', [
        block('logic_compare'),
        block('logic_compare_strict', { A: txt('10'), B: txt('9') }),
        block('logic_is_null'),
        block('logic_operation', { A: bool(), B: bool() }),
        block('logic_negate', { BOOL: bool() }),
        block('logic_boolean'),
        block('logic_ternary', { IF: bool() }),
        block('logic_or_default', { B: txt('default') }),
      ]),
      cat('Math', 'math', [
        block('math_number'),
        block('math_arithmetic', { A: num(1), B: num(1) }),
        block('math_single', { NUM: num(9) }),
        block('math_round', { NUM: num(3.1) }),
        block('math_round_ha', { NUM: num(2.5) }),
        block('to_number_ha', { VALUE: txt('21.5') }),
        block('math_modulo', { DIVIDEND: num(64), DIVISOR: num(10) }),
        block('math_constrain', { VALUE: num(50), LOW: num(1), HIGH: num(100) }),
        block('math_between', { VALUE: num(21), LOW: num(18), HIGH: num(24) }),
        block('math_random_int', { FROM: num(1), TO: num(100) }),
        block('math_on_list'),
        block('math_number_property', { NUMBER_TO_CHECK: num(0) }),
      ]),
      cat('Text', 'text', [
        block('text'),
        block('text_join'),
        block('text_length', { VALUE: txt('abc') }),
        block('text_isEmpty', { VALUE: txt('') }),
        block('text_contains', { FIND: txt('a') }),
        block('text_split_at', { SEP: txt(':'), TEXT: txt('mode: night') }),
        block('text_changeCase', { TEXT: txt('abc') }),
        block('text_trim', { TEXT: txt(' abc ') }),
        block('text_to_number', { TEXT: txt('42') }),
        block('to_text'),
        block('text_append', { TEXT: txt('') }),
        block('text_regex_test', { TEXT: txt('battery 42%') }),
        block('text_regex', { TEXT: txt('battery 42%') }),
      ]),
      sep,
      cat('Variables', 'variable', [], { custom: 'VARIABLE' }),
      cat('Stored', 'stored', [], { custom: 'STORED_VARIABLES' }),
      cat('Lists', 'list', [
        block('lists_create_with'),
        block('lists_length'),
        block('lists_isEmpty'),
        block('lists_getIndex'),
        block('lists_setIndex'),
        block('lists_indexOf'),
        block('lists_sort'),
        block('lists_split', { DELIM: txt(',') }),
        block('lists_reverse'),
        block('lists_repeat', { NUM: num(5) }),
        block('lists_filter', { COND: bool() }),
        block('lists_map'),
        block('lists_item'),
      ]),
      cat('Functions', 'procedure', [], { custom: 'PROCEDURE' }),
      cat('Time', 'time', [
        block('time_now'),
        block('time_of_day'),
        block('time_part'),
        block('time_today_at'),
        block('duration', { N: num(5) }),
        block('time_add', { DATETIME: { block: { type: 'time_now' } }, SECONDS: dur(1, 'hours') }),
        block('time_seconds_between', { A: { block: { type: 'time_now' } } }),
        block('format_duration', { SECONDS: num(3725) }),
        block('time_format', { DATETIME: { block: { type: 'time_now' } } }),
        block('time_is_between'),
      ]),
      ...(extra.length ? [sep, ...extra.map(extraToItem)] : []),
      sep,
      cat('Advanced', 'http', [
        label('Home Assistant, the technical way'),
        block('ha_call_service', {
          TARGET: ent(),
          DATA: { block: { type: 'dict_create', inputs: { ITEMS: { block: { type: 'dict_pair', fields: { KEY: 'brightness_pct' }, inputs: { VALUE: num(80) } } } } } },
        }),
        block('ha_call_service_response', {
          TARGET: ent(),
          DATA: { block: { type: 'dict_create', inputs: { ITEMS: { block: { type: 'dict_pair', fields: { KEY: 'type' }, inputs: { VALUE: txt('daily') } } } } } },
        }),
        block('ha_fire_event', { EVENT_TYPE: txt('my_event') }),
        block('ha_trigger_template', { FOR: dur(0) }),
        block('ha_trigger_raw'),
        block('ha_action_raw'),
        block('ha_condition_raw'),
        block('ha_template'),
        block('ha_template_condition'),
        block('ha_trigger_data'),
        block('to_text_ha'),
        block('yaml_value'),
        label('Schedules, webhooks & network'),
        block('engine_trigger_cron'),
        block('engine_trigger_webhook'),
        block('engine_trigger_mqtt'),
        block('ha_trigger_webhook'),
        block('ha_trigger_mqtt'),
        block('mqtt_publish', { TOPIC: txt('home/test'), PAYLOAD: txt('hello') }),
        block('http_request', { URL: txt('https://example.com/api') }),
        block('http_send', { URL: txt('https://example.com/api') }),
        label('Dictionaries & JSON'),
        block('dict_create', { ITEMS: { block: { type: 'dict_pair' } } }),
        block('dict_pair'),
        block('dict_get', { KEY: txt('key') }),
        block('dict_set', { KEY: txt('key') }),
        block('dict_has', { KEY: txt('key') }),
        block('dict_keys'),
        block('dict_values'),
        block('json_parse'),
        block('json_stringify'),
        label('Errors & parallel runs'),
        block('control_try'),
        block('control_throw', { MESSAGE: txt('Something went wrong') }),
        block('control_parallel', {
          BRANCHES: { block: { type: 'control_branch', next: { block: { type: 'control_branch' } } } },
        }),
        block('control_branch'),
        block('logic_null'),
      ]),
    ],
  };
}

/** Flyout for persistent variables (typed 'Program' or 'Global'). */
export function registerStoredVariablesCategory(ws: Blockly.WorkspaceSvg): void {
  const create = (type: 'Program' | 'Global') => (button: Blockly.FlyoutButton) =>
    Blockly.Variables.createVariableButtonHandler(button.getTargetWorkspace(), undefined, type);
  ws.registerButtonCallback('CREATE_PROGRAM_VAR', create('Program'));
  ws.registerButtonCallback('CREATE_GLOBAL_VAR', create('Global'));

  ws.registerToolboxCategoryCallback('STORED_VARIABLES', (workspace) => {
    const items: Blockly.utils.toolbox.FlyoutItemInfo[] = [
      { kind: 'label', text: 'Kept across restarts' },
      { kind: 'button', text: 'New program variable…', callbackkey: 'CREATE_PROGRAM_VAR' },
      { kind: 'button', text: 'New global variable (shared)…', callbackkey: 'CREATE_GLOBAL_VAR' },
    ];
    const vars = [
      ...workspace.getVariableMap().getVariablesOfType('Program'),
      ...workspace.getVariableMap().getVariablesOfType('Global'),
    ];
    for (const v of vars) {
      const ref = { id: v.getId() };
      items.push({ kind: 'block', type: 'variables_set', fields: { VAR: ref } } as Blockly.utils.toolbox.FlyoutItemInfo);
      items.push({ kind: 'block', type: 'variables_get', fields: { VAR: ref } } as Blockly.utils.toolbox.FlyoutItemInfo);
    }
    return items;
  });
}
