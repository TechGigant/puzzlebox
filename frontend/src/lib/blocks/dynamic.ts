//! Renders registry block definitions (HA autogen, packs, plugins) as Blockly blocks.

import * as Blockly from 'blockly/core';
import type { ExtraCategory } from '../toolbox';
import { ENTITY_DOMAINS, HELPER_DOMAINS } from './helpers';
import { EVERYDAY_DOMAINS } from './everyday';
import { FieldOptions, FieldRegistry, FieldStateValue, setDomainResolver } from './field_entity';
import { addTriggerSettings, chooseTriggerSetting, triggerSettingItems } from './trigger_settings';

export interface SelectOption {
  label: string;
  value: string;
}

export type ParamInput =
  | { input: 'number'; min?: number; max?: number; step?: number; unit?: string }
  | { input: 'text'; multiline?: boolean }
  | { input: 'boolean' }
  | { input: 'select'; options: SelectOption[]; multiple?: boolean }
  | { input: 'entity'; domains: string[]; multiple?: boolean; device_classes?: string[] }
  | { input: 'registry'; kind: string; multiple?: boolean }
  | { input: 'duration' }
  | { input: 'time' }
  | { input: 'color' }
  | { input: 'threshold'; mode: string; units: (string | null)[] }
  | { input: 'state_value'; attribute?: string; multiple: boolean }
  | { input: 'any' };

export type Param = ParamInput & {
  name: string;
  label: string;
  description?: string;
  required?: boolean;
  default?: unknown;
};

export interface BlockDef {
  type: string;
  kind: 'action' | 'trigger' | 'condition' | 'value';
  category: string[];
  style: string;
  title: string;
  tooltip?: string;
  help_url?: string;
  target?: { domains: string[]; device_classes?: string[]; required: boolean };
  params: Param[];
  source: string;
}

const defs = new Map<string, BlockDef>();

export function blockDef(type: string): BlockDef | undefined {
  return defs.get(type);
}

export function allBlockDefs(): Iterable<BlockDef> {
  return defs.values();
}

const slot = (p: Param) => `P_${p.name}`;

const REMOVE_ICON =
  'data:image/svg+xml;utf8,' +
  encodeURIComponent(
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path fill="white" d="M19 6.4 17.6 5 12 10.6 6.4 5 5 6.4 10.6 12 5 17.6 6.4 19 12 13.4 17.6 19 19 17.6 13.4 12z"/></svg>',
  );



const THRESHOLD_TYPES: [string, string][] = [
  ['above', 'above'],
  ['below', 'below'],
  ['between', 'between'],
  ['outside', 'outside'],
];

/** Helper blocks whose target can't be a whole area (it isn't passed on as an action target). */
const NO_AREA_TARGET = new Set(['media_say', 'scene_activate', 'script_run', 'counter_command']);

setDomainResolver((parent, inputName) => {
  const def = defs.get(parent.type);
  // Light helper blocks only take lights.
  if (!def && inputName === 'TARGET') {
    if (parent.type.startsWith('light_')) return { domains: ['light'], areas: true };
    if (HELPER_DOMAINS[parent.type]) return { domains: HELPER_DOMAINS[parent.type], areas: !NO_AREA_TARGET.has(parent.type) };
    if (EVERYDAY_DOMAINS[parent.type]) return { domains: EVERYDAY_DOMAINS[parent.type] };
  }
  if (!def && inputName === 'ENTITY' && EVERYDAY_DOMAINS[parent.type]) return { domains: EVERYDAY_DOMAINS[parent.type] };
  if (!def && inputName === 'ENTITY' && ENTITY_DOMAINS[parent.type]) return { domains: ENTITY_DOMAINS[parent.type] };
  if (!def) return { domains: [] };
  if (inputName === 'TARGET') return { domains: def.target?.domains ?? [], deviceClasses: def.target?.device_classes, areas: true };
  const p = def.params.find((x) => slot(x) === inputName);
  return p?.input === 'entity' ? { domains: p.domains, deviceClasses: p.device_classes } : { domains: [] };
});

/** Durations, and numbers of seconds (transitions, delays), take a "5 minutes" block. */
export function secondsParam(p: Param): boolean {
  return p.input === 'duration' || (p.input === 'number' && p.unit === 'seconds');
}

function shadowFor(p: Param): Blockly.serialization.blocks.State | null {
  const d = p.default;
  if (secondsParam(p)) return durationShadow(typeof d === 'number' ? d : 0);
  switch (p.input) {
    case 'number':
      return { type: 'math_number', fields: { NUM: typeof d === 'number' ? d : (p.min ?? 0) } };
    case 'text':
    case 'time':
      return { type: 'text', fields: { TEXT: typeof d === 'string' ? d : '' } };
    case 'entity':
      return { type: 'ha_entity', fields: { ENTITY: typeof d === 'string' ? d : '' } };
    case 'color':
      // A socket, so colour blocks (mix, random, colour of a light…) fit too.
      return { type: 'colour_picker', fields: { COLOUR: typeof d === 'string' ? d : '#ffcc66' } };
    case 'any':
      // Lists, dictionaries and other structured values; empty means "not set".
      return { type: 'yaml_value', fields: { CONFIG: d == null ? '' : typeof d === 'string' ? d : JSON.stringify(d) } };
    default:
      return null;
  }
}

/** A "5 minutes" block holding a number of seconds, in the largest whole unit. */
export function durationShadow(seconds: number): Blockly.serialization.blocks.State {
  const [unit, n] = seconds && seconds % 3600 === 0 ? ['hours', seconds / 3600] : seconds && seconds % 60 === 0 ? ['minutes', seconds / 60] : ['seconds', seconds];
  return { type: 'duration', fields: { UNIT: unit }, inputs: { N: { shadow: { type: 'math_number', fields: { NUM: n } } } } };
}

/** HA's labels are title-like ("Trigger when"); ours read as a sentence ("trigger when"). Acronyms stay. */
export function sentence(label: string): string {
  return /^[A-Z][a-z]/.test(label) ? label[0].toLowerCase() + label.slice(1) : label;
}

function labelFor(p: Param): string {
  if (p.input === 'number' && p.unit && !secondsParam(p)) return `${sentence(p.label)} (${p.unit})`;
  return sentence(p.label);
}

type DynBlock = Blockly.BlockSvg & { hab: BlockDef; optional: string[] };

function appendParam(block: DynBlock, p: Param, withShadow: boolean, removable: boolean) {
  const name = slot(p);
  let input: Blockly.Input;
  switch (p.input) {
    case 'select': {
      if (p.multiple) {
        const d = Array.isArray(p.default) ? p.default.join(', ') : typeof p.default === 'string' ? p.default : '';
        const field = new FieldOptions(d, p.options.map((o) => ({ value: o.value, label: o.label })));
        input = block.appendDummyInput(name).appendField(labelFor(p)).appendField(field, name);
        break;
      }
      const options = p.options.map((o) => [o.label || o.value, o.value] as [string, string]);
      const field = new Blockly.FieldDropdown(options);
      if (typeof p.default === 'string' && p.options.some((o) => o.value === p.default)) field.setValue(p.default);
      input = block.appendDummyInput(name).appendField(labelFor(p)).appendField(field, name);
      break;
    }
    case 'boolean':
      input = block
        .appendDummyInput(name)
        .appendField(labelFor(p))
        .appendField(new Blockly.FieldCheckbox(p.default === true ? 'TRUE' : 'FALSE'), name);
      break;
    case 'registry':
      input = block
        .appendDummyInput(name)
        .appendField(labelFor(p))
        .appendField(new FieldRegistry(typeof p.default === 'string' ? p.default : '', p.kind), name);
      break;
    case 'state_value': {
      const field = new FieldStateValue(typeof p.default === 'string' ? p.default : '');
      field.attribute = p.attribute ?? null;
      input = block.appendDummyInput(name).appendField(labelFor(p)).appendField(field, name);
      break;
    }
    case 'threshold': {
      // One HA value, several editor parts: type, optional unit, and one or two numbers.
      const types = p.mode === 'changed' ? [...THRESHOLD_TYPES, ['any value', 'any'] as [string, string]] : THRESHOLD_TYPES;
      // Only "between" and "outside" use the second number; "any value" uses none.
      const showInputs = (type: string) => {
        block.getInput(`${name}__a`)?.setVisible(type !== 'any');
        block.getInput(`${name}__b`)?.setVisible(type === 'between' || type === 'outside');
      };
      const typeField = new Blockly.FieldDropdown(types, (v) => {
        if (v) setTimeout(() => (showInputs(v), block.rendered && block.render()));
        return v;
      });
      input = block.appendDummyInput(name).appendField(labelFor(p)).appendField(typeField, name);
      if (p.units.length) {
        const units = p.units.map((u) => [u ?? 'no unit', u ?? '__none__'] as [string, string]);
        input.appendField(new Blockly.FieldDropdown(units), `${name}__unit`);
      }
      for (const [suffix, label] of [['a', 'value'], ['b', 'and']]) {
        const v = block.appendValueInput(`${name}__${suffix}`).appendField(label).setCheck('Number');
        v.setAlign(Blockly.inputs.Align.RIGHT);
        if (withShadow) v.connection?.setShadowState({ type: 'math_number', fields: { NUM: suffix === 'a' ? 0 : 100 } });
        if (block.getInput('ADD')) block.moveInputBefore(v.name, 'ADD');
      }
      showInputs(typeField.getValue() ?? 'above');
      break;
    }
    default:
      input = block.appendValueInput(name).appendField(labelFor(p));
      input.setAlign(Blockly.inputs.Align.RIGHT);
      if (withShadow) {
        const s = shadowFor(p);
        if (s) input.connection?.setShadowState(s);
      }
  }
  // Labels line up against their values when the block has a row per setting.
  input.setAlign(Blockly.inputs.Align.RIGHT);
  if (removable) {
    input.appendField(new Blockly.FieldImage(REMOVE_ICON, 14, 14, 'remove', () => removeOptional(block, p.name)));
  }
  if (block.getInput('ADD')) block.moveInputBefore(name, 'ADD');
  if (p.description) {
    // Per-input help isn't supported by Blockly; fold it into the block tooltip instead.
  }
}

/** Short blocks read as one line, like the built-in ones; longer ones get a row per setting. */
function relayout(block: DynBlock) {
  const shown = block.hab.params.filter((p) => p.required || block.optional.includes(p.name));
  const tall = shown.some((p) => p.input === 'threshold' || (p.input === 'text' && p.multiline));
  block.setInputsInline(!tall && shown.length + (block.hab.target ? 1 : 0) <= 2);
}

/** The whole extra state, which undo restores (it also has the trigger settings). */
const extraState = (block: DynBlock) => JSON.stringify(block.saveExtraState?.() ?? null);

function fireMutation(block: DynBlock, before: string) {
  const after = extraState(block);
  if (before === after) return;
  const Change = Blockly.Events.get(Blockly.Events.BLOCK_CHANGE);
  Blockly.Events.fire(new Change(block, 'mutation', null, before, after));
}

function addOptional(block: DynBlock, name: string) {
  const p = block.hab.params.find((x) => x.name === name);
  if (!p || block.optional.includes(name)) return;
  const before = extraState(block);
  block.optional.push(name);
  appendParam(block, p, true, true);
  relayout(block);
  fireMutation(block, before);
}

function removeOptional(block: DynBlock, name: string) {
  const p = block.hab.params.find((x) => x.name === name);
  if (!p) return;
  const before = extraState(block);
  block.optional = block.optional.filter((n) => n !== name);
  for (const input of [...block.inputList]) {
    if (input.name === slot(p) || input.name.startsWith(`${slot(p)}__`)) block.removeInput(input.name, true);
  }
  relayout(block);
  fireMutation(block, before);
}

function tooltip(def: BlockDef): string {
  const lines = [def.tooltip ?? ''];
  for (const p of def.params) if (p.description) lines.push(`• ${p.label}: ${p.description}`);
  return lines.filter(Boolean).join('\n');
}

export function defineBlock(def: BlockDef) {
  defs.set(def.type, def);
  const optionalParams = def.params.filter((p) => !p.required);
  // Home Assistant's own triggers can have an id and name; plugin triggers may not be HA's.
  const settings = def.kind === 'trigger' && def.source === 'ha';
  Blockly.Blocks[def.type] = {
    init(this: DynBlock) {
      this.hab = def;
      this.optional = [];
      // Lower-case like the built-in blocks ("turn on light"), but keep acronyms ("MQTT").
      const title = sentence(def.title);
      this.appendDummyInput('TITLE').appendField(def.kind === 'trigger' ? `⚡ ${title}` : title);
      if (def.target) {
        const verb = def.kind === 'action' ? 'on' : 'for';
        this.appendValueInput('TARGET').appendField(verb).setAlign(Blockly.inputs.Align.RIGHT);
      }
      for (const p of def.params.filter((x) => x.required)) appendParam(this, p, false, false);
      if (optionalParams.length) {
        const menu = (): [string, string][] => [
          ['＋ more settings', '__add__'],
          ...optionalParams.filter((p) => !this.optional.includes(p.name)).map((p) => [labelFor(p), p.name] as [string, string]),
          ...(settings ? triggerSettingItems(this) : []),
        ];
        const dd = new Blockly.FieldDropdown(menu, (value) => {
          if (value && value !== '__add__') setTimeout(() => (settings && chooseTriggerSetting(this, value)) || addOptional(this, value));
          return null;
        });
        this.appendDummyInput('ADD').appendField(dd, 'ADD_OPTION');
      }
      this.setStyle(`${def.style}_blocks`);
      this.setTooltip(tooltip(def));
      if (def.help_url) this.setHelpUrl(def.help_url);
      relayout(this);
      switch (def.kind) {
        case 'action':
          this.setPreviousStatement(true);
          this.setNextStatement(true);
          break;
        case 'trigger':
          this.setPreviousStatement(true, 'Trigger');
          this.setNextStatement(true, 'Trigger');
          if (settings) addTriggerSettings(this, this.getInput('ADD') ? 'ADD' : undefined);
          break;
        case 'condition':
          this.setOutput(true, 'Boolean');
          break;
        case 'value':
          this.setOutput(true);
          break;
      }
    },
    saveExtraState(this: DynBlock) {
      return this.optional.length ? { optional: [...this.optional] } : null;
    },
    loadExtraState(this: DynBlock, state: { optional?: string[] }) {
      for (const name of state?.optional ?? []) {
        const p = def.params.find((x) => x.name === name);
        if (p && !this.optional.includes(name)) {
          this.optional.push(name);
          appendParam(this, p, false, true);
        }
      }
      relayout(this);
    },
  };
}

function toolboxItem(def: BlockDef): Blockly.utils.toolbox.ToolboxItemInfo {
  const inputs: Record<string, unknown> = {};
  if (def.target) inputs.TARGET = { shadow: { type: 'ha_entity', fields: { ENTITY: '' } } };
  for (const p of def.params.filter((x) => x.required)) {
    if (p.input === 'threshold') {
      inputs[`${slot(p)}__a`] = { shadow: { type: 'math_number', fields: { NUM: 0 } } };
      inputs[`${slot(p)}__b`] = { shadow: { type: 'math_number', fields: { NUM: 100 } } };
      continue;
    }
    const s = shadowFor(p);
    if (s) inputs[slot(p)] = { shadow: s };
  }
  return { kind: 'block', type: def.type, inputs } as Blockly.utils.toolbox.ToolboxItemInfo;
}

/** Built-in blocks that do what a Home Assistant domain's generated blocks do, more simply. */
const CURATED: Record<string, string[]> = {
  light: ['light_power', 'light_set_colour', 'light_set_white', 'light_set_brightness', 'light_step', 'light_fade'],
  switch: ['device_power', 'device_on_for'],
  homeassistant: ['device_power'],
  cover: ['cover_command', 'cover_set_position'],
  climate: ['climate_set_temperature', 'climate_set_mode', 'climate_set_preset'],
  media_player: ['media_command', 'media_set_volume', 'media_play', 'media_say'],
  fan: ['fan_set_speed', 'device_power'],
  lock: ['lock_command'],
  vacuum: ['vacuum_command'],
  scene: ['scene_activate'],
  script: ['script_run'],
  counter: ['counter_command'],
  notify: ['notify_send'],
  input_boolean: ['helper_set'],
  input_number: ['helper_set'],
  input_select: ['helper_set'],
  input_text: ['helper_set'],
  input_datetime: ['helper_set'],
};

/** The domain of a generated block (`ha_svc__light__turn_on` → `light`). */
const domainOf = (def: BlockDef) => def.type.split('__')[1] ?? '';

const KIND_ORDER: BlockDef['kind'][] = ['trigger', 'condition', 'action', 'value'];
const KIND_LABEL: Record<BlockDef['kind'], string> = {
  trigger: 'Triggers',
  condition: 'Conditions',
  action: 'Actions',
  value: 'Values',
};

function categoryContents(blocks: BlockDef[]): Blockly.utils.toolbox.ToolboxItemInfo[] {
  const out: Blockly.utils.toolbox.ToolboxItemInfo[] = [];
  for (const kind of KIND_ORDER) {
    const of = blocks.filter((b) => b.kind === kind).sort((a, b) => a.title.localeCompare(b.title));
    if (!of.length) continue;
    out.push({ kind: 'label', text: KIND_LABEL[kind] } as Blockly.utils.toolbox.ToolboxItemInfo);
    out.push(...of.map(toolboxItem));
  }
  return out;
}

/** Defines every block and returns toolbox categories plus theme style names. */
export function installRegistry(blocks: BlockDef[]): { categories: ExtraCategory[]; styles: string[] } {
  defs.clear();
  for (const b of blocks) defineBlock(b);

  const group = (list: BlockDef[], depth = 0) => {
    const by = new Map<string, BlockDef[]>();
    for (const b of list) {
      const key = b.category[depth] ?? 'Other';
      by.set(key, [...(by.get(key) ?? []), b]);
    }
    return [...by.entries()].sort(([a], [b]) => a.localeCompare(b));
  };
  // Categories nest when blocks have a second category level (e.g. Zigbee2MQTT → device).
  const tree = (name: string, list: BlockDef[], depth: number): ExtraCategory => {
    const flat = list.filter((b) => b.category.length <= depth + 1);
    const nested = list.filter((b) => b.category.length > depth + 1);
    return {
      name,
      style: list[0].style,
      blocks: categoryContents(flat),
      subcategories: group(nested, depth + 1).map(([n, l]) => tree(n, l, depth + 1)),
    };
  };

  const ha = blocks.filter((b) => b.source === 'ha');
  const others = blocks.filter((b) => b.source !== 'ha');
  const categories: ExtraCategory[] = [];
  if (ha.length) {
    categories.push({
      name: 'Home Assistant',
      style: 'action',
      blocks: [],
      subcategories: group(ha).map(([name, list]) => ({
        name,
        style: 'action',
        blocks: categoryContents(list),
        curated: [...new Set(list.flatMap((b) => CURATED[domainOf(b)] ?? []))],
      })),
    });
  }
  for (const [name, list] of group(others)) categories.push(tree(name, list, 0));
  const styles = [...new Set(blocks.map((b) => b.style))];
  return { categories, styles };
}
