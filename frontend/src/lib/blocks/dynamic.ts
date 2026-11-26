//! Renders registry block definitions (HA autogen, packs, plugins) as Blockly blocks.

import * as Blockly from 'blockly/core';
import { FieldColour } from '@blockly/field-colour';
import type { ExtraCategory } from '../toolbox';
import { FieldOptions, FieldRegistry, FieldStateValue, setDomainResolver, setTargetResolver } from './field_entity';

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

setTargetResolver((block) => {
  const target = block.getInputTargetBlock('TARGET');
  const raw = target?.type === 'ha_entity' ? String(target.getFieldValue('ENTITY') ?? '') : '';
  return raw
    .split(',')
    .map((s) => s.trim())
    .filter(Boolean);
});

const THRESHOLD_TYPES: [string, string][] = [
  ['above', 'above'],
  ['below', 'below'],
  ['between', 'between'],
  ['outside', 'outside'],
];

setDomainResolver((parent, inputName) => {
  const def = defs.get(parent.type);
  // Light helper blocks only take lights.
  if (!def && parent.type.startsWith('light_') && inputName === 'TARGET') return { domains: ['light'] };
  if (!def) return { domains: [] };
  if (inputName === 'TARGET') return { domains: def.target?.domains ?? [], deviceClasses: def.target?.device_classes };
  const p = def.params.find((x) => slot(x) === inputName);
  return p?.input === 'entity' ? { domains: p.domains, deviceClasses: p.device_classes } : { domains: [] };
});

function shadowFor(p: Param): Blockly.serialization.blocks.State | null {
  const d = p.default;
  switch (p.input) {
    case 'number':
    case 'duration':
      return { type: 'math_number', fields: { NUM: typeof d === 'number' ? d : (p.input === 'number' ? (p.min ?? 0) : 0) } };
    case 'text':
    case 'time':
      return { type: 'text', fields: { TEXT: typeof d === 'string' ? d : '' } };
    case 'entity':
      return { type: 'ha_entity', fields: { ENTITY: typeof d === 'string' ? d : '' } };
    case 'any':
      // Lists, dictionaries and other structured values; empty means "not set".
      return { type: 'yaml_value', fields: { CONFIG: d == null ? '' : typeof d === 'string' ? d : JSON.stringify(d) } };
    default:
      return null;
  }
}

function labelFor(p: Param): string {
  if (p.input === 'number' && p.unit) return `${p.label} (${p.unit})`;
  if (p.input === 'duration') return `${p.label} (s)`;
  return p.label;
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
        input = block.appendDummyInput(name).appendField(p.label).appendField(field, name);
        break;
      }
      const options = p.options.map((o) => [o.label || o.value, o.value] as [string, string]);
      const field = new Blockly.FieldDropdown(options);
      if (typeof p.default === 'string' && p.options.some((o) => o.value === p.default)) field.setValue(p.default);
      input = block.appendDummyInput(name).appendField(p.label).appendField(field, name);
      break;
    }
    case 'boolean':
      input = block
        .appendDummyInput(name)
        .appendField(p.label)
        .appendField(new Blockly.FieldCheckbox(p.default === true ? 'TRUE' : 'FALSE'), name);
      break;
    case 'registry':
      input = block
        .appendDummyInput(name)
        .appendField(p.label)
        .appendField(new FieldRegistry(typeof p.default === 'string' ? p.default : '', p.kind), name);
      break;
    case 'color':
      input = block.appendDummyInput(name).appendField(p.label).appendField(new FieldColour('#ffcc66'), name);
      break;
    case 'state_value': {
      const field = new FieldStateValue(typeof p.default === 'string' ? p.default : '');
      field.attribute = p.attribute ?? null;
      input = block.appendDummyInput(name).appendField(p.label).appendField(field, name);
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
      input = block.appendDummyInput(name).appendField(p.label).appendField(typeField, name);
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
  if (removable) {
    input.appendField(new Blockly.FieldImage(REMOVE_ICON, 14, 14, 'remove', () => removeOptional(block, p.name)));
  }
  if (block.getInput('ADD')) block.moveInputBefore(name, 'ADD');
  if (p.description) {
    // Per-input help isn't supported by Blockly; fold it into the block tooltip instead.
  }
}

function fireMutation(block: DynBlock, before: string) {
  const after = JSON.stringify({ optional: block.optional });
  if (before === after) return;
  const Change = Blockly.Events.get(Blockly.Events.BLOCK_CHANGE);
  Blockly.Events.fire(new Change(block, 'mutation', null, before, after));
}

function addOptional(block: DynBlock, name: string) {
  const p = block.hab.params.find((x) => x.name === name);
  if (!p || block.optional.includes(name)) return;
  const before = JSON.stringify({ optional: block.optional });
  block.optional.push(name);
  appendParam(block, p, true, true);
  fireMutation(block, before);
}

function removeOptional(block: DynBlock, name: string) {
  const p = block.hab.params.find((x) => x.name === name);
  if (!p) return;
  const before = JSON.stringify({ optional: block.optional });
  block.optional = block.optional.filter((n) => n !== name);
  for (const input of [...block.inputList]) {
    if (input.name === slot(p) || input.name.startsWith(`${slot(p)}__`)) block.removeInput(input.name, true);
  }
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
  Blockly.Blocks[def.type] = {
    init(this: DynBlock) {
      this.hab = def;
      this.optional = [];
      this.appendDummyInput('TITLE').appendField(def.title);
      if (def.target) {
        const verb = def.kind === 'action' ? 'on' : 'for';
        this.appendValueInput('TARGET').appendField(verb).setAlign(Blockly.inputs.Align.RIGHT);
      }
      for (const p of def.params.filter((x) => x.required)) appendParam(this, p, false, false);
      if (optionalParams.length) {
        const menu = (): [string, string][] => [
          ['＋ option', '__add__'],
          ...optionalParams.filter((p) => !this.optional.includes(p.name)).map((p) => [p.label, p.name] as [string, string]),
        ];
        const dd = new Blockly.FieldDropdown(menu, (value) => {
          if (value && value !== '__add__') setTimeout(() => addOptional(this, value));
          return null;
        });
        this.appendDummyInput('ADD').appendField(dd, 'ADD_OPTION');
      }
      this.setStyle(`${def.style}_blocks`);
      this.setTooltip(tooltip(def));
      if (def.help_url) this.setHelpUrl(def.help_url);
      this.setInputsInline(false);
      switch (def.kind) {
        case 'action':
          this.setPreviousStatement(true);
          this.setNextStatement(true);
          break;
        case 'trigger':
          this.setPreviousStatement(true, 'Trigger');
          this.setNextStatement(true, 'Trigger');
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
      subcategories: group(ha).map(([name, list]) => ({ name, style: 'action', blocks: categoryContents(list) })),
    });
  }
  for (const [name, list] of group(others)) categories.push(tree(name, list, 0));
  const styles = [...new Set(blocks.map((b) => b.style))];
  return { categories, styles };
}
