//! Home Assistant condition blocks (numeric state, sun, zone, template) and the optional
//! settings of the state conditions. Each compiles to one native HA condition config.

import * as Blockly from 'blockly/core';
import { FieldAttribute, FieldStateValue } from './field_entity';

type Json = Record<string, unknown>;

const value = (name: string, check?: string) => ({ type: 'input_value', name, ...(check ? { check } : {}) });
const dropdown = (name: string, options: [string, string][]) => ({ type: 'field_dropdown', name, options });
const text = (name: string, v = '') => ({ type: 'field_input', name, text: v });
const entity = (name: string, v = '', domains: string[] = []) => ({ type: 'field_entity', name, text: v, domains });
const multiline = (name: string, v = '') => ({ type: 'field_multilinetext', name, text: v, spellcheck: false });

const SUN_WHEN: [string, string][] = [
  ['after', 'after'],
  ['before', 'before'],
];
const SUN_EVENT: [string, string][] = [
  ['sunset', 'sunset'],
  ['sunrise', 'sunrise'],
];

interface OptionRow {
  name: string;
  /** Menu text for adding it. */
  label: string;
  /** Appends the row's input (named `name`) to the block. */
  build: (block: Blockly.Block, input: (name: string) => Blockly.Input) => void;
  /** Shadow blocks for a row added from the menu (loading a program brings its own). */
  shadows?: Record<string, Blockly.serialization.blocks.State>;
}

const attributeRow: OptionRow = {
  name: 'ATTRIBUTE',
  label: 'compare a detail (attribute)',
  build: (block, input) => {
    input('dummy').appendField('using its detail').appendField(new FieldAttribute(''), 'ATTRIBUTE');
    // The state box then suggests values of that detail.
    const state = block.getField('STATE');
    const attr = block.getField('ATTRIBUTE');
    if (state instanceof FieldStateValue && attr) {
      state.attribute = String(attr.getValue() ?? '') || null;
      attr.setValidator((v) => {
        state.attribute = String(v ?? '') || null;
        return v;
      });
    }
  },
};

const matchRow: OptionRow = {
  name: 'MATCH',
  label: 'several entities: all or any',
  build: (_block, input) => {
    input('dummy')
      .appendField('with several entities:')
      .appendField(
        new Blockly.FieldDropdown([
          ['all of them', 'all'],
          ['any of them', 'any'],
        ]),
        'MATCH',
      );
  },
};

const OPTIONS: Record<string, OptionRow[]> = {
  ha_is_state: [attributeRow, matchRow],
  ha_state_for: [matchRow],
  ha_numeric_state: [
    attributeRow,
    {
      name: 'VALUE_TEMPLATE',
      label: 'value from a template',
      build: (_block, input) => {
        input('dummy').appendField('value from template').appendField(new Blockly.FieldTextInput('{{ state.attributes.value }}'), 'VALUE_TEMPLATE');
      },
    },
  ],
  sun_condition: [
    {
      name: 'SECOND',
      label: 'a second limit',
      build: (_block, input) => {
        input('value')
          .setCheck('Number')
          .appendField('and')
          .appendField(new Blockly.FieldDropdown(SUN_WHEN), 'WHEN2')
          .appendField(new Blockly.FieldDropdown(SUN_EVENT), 'EVENT2')
          .appendField('shifted by');
      },
      shadows: { SECOND: { type: 'duration', inputs: { N: { shadow: { type: 'math_number', fields: { NUM: 0 } } } }, fields: { UNIT: 'minutes' } } },
    },
  ],
};

/** The optional settings a block type has (kept when switching between blocks). */
export function optionNames(type: string): string[] {
  return (OPTIONS[type] ?? []).map((o) => o.name);
}

type OptBlock = Blockly.Block & { options: string[] };

const MENU = '__menu__';

/** A menu, not a setting: the chosen settings are saved in the extra state. */
class MenuField extends Blockly.FieldDropdown {
  override isSerializable() {
    return false;
  }
}

function rowsOf(block: OptBlock): OptionRow[] {
  return OPTIONS[block.type] ?? [];
}

function fireMutation(block: OptBlock, before: string) {
  const after = JSON.stringify({ options: block.options });
  if (before === after) return;
  const Change = Blockly.Events.get(Blockly.Events.BLOCK_CHANGE);
  Blockly.Events.fire(new Change(block, 'mutation', null, before, after));
}

function addRow(block: OptBlock, name: string, fromMenu: boolean) {
  const row = rowsOf(block).find((r) => r.name === name);
  if (!row || block.options.includes(name)) return;
  block.options.push(name);
  // Ends the line before, so each setting gets its own line.
  block.appendEndRowInput(`${name}__row`);
  row.build(block, (kind) => (kind === 'value' ? block.appendValueInput(name) : block.appendDummyInput(name)));
  block.moveInputBefore(`${name}__row`, 'OPTIONS');
  block.moveInputBefore(name, 'OPTIONS');
  if (fromMenu) {
    for (const [input, shadow] of Object.entries(row.shadows ?? {})) block.getInput(input)?.connection?.setShadowState(shadow);
  }
}

function removeRow(block: OptBlock, name: string) {
  if (!block.options.includes(name)) return;
  block.options = block.options.filter((n) => n !== name);
  block.removeInput(name, true);
  block.removeInput(`${name}__row`, true);
}

const mixin = {
  saveExtraState(this: OptBlock) {
    return this.options.length ? { options: [...this.options] } : null;
  },
  // Also used by undo/redo, so it removes rows that aren't in the state.
  loadExtraState(this: OptBlock, state: { options?: string[] } | null) {
    const wanted = state?.options ?? [];
    for (const name of this.options.filter((n) => !wanted.includes(n))) removeRow(this, name);
    for (const name of wanted) addRow(this, name, false);
  },
};

function helper(this: OptBlock) {
  this.options = [];
  const menu = (): [string, string][] => [
    ['＋ option', MENU],
    ...rowsOf(this).map((r): [string, string] => (this.options.includes(r.name) ? [`− remove: ${r.label}`, `-${r.name}`] : [`＋ ${r.label}`, `+${r.name}`])),
  ];
  const field = new MenuField(menu, (v) => {
    if (v && v !== MENU) {
      setTimeout(() => {
        const before = JSON.stringify({ options: this.options });
        if (v.startsWith('+')) addRow(this, v.slice(1), true);
        else removeRow(this, v.slice(1));
        fireMutation(this, before);
      });
    }
    return null;
  });
  this.appendDummyInput('OPTIONS').appendField(field, 'OPTIONS_MENU');
}

export const CONDITION_BLOCKS: Json[] = [
  {
    type: 'ha_numeric_state',
    message0: '%1 is above %2 and below %3',
    args0: [value('ENTITY'), text('ABOVE', '20'), text('BELOW')],
    output: 'Boolean',
    style: 'condition_blocks',
    inputsInline: true,
    mutator: 'pb_condition_options',
    tooltip:
      'True while the value is in this range (the limits themselves don\'t count). Leave one side empty for just "above" or just "below"; a side can also be a number helper or sensor (input_number.…). False while the entity is unavailable. With several entities, all must be in range.',
  },
  {
    type: 'sun_condition',
    message0: 'it is %1 %2 shifted by %3',
    args0: [dropdown('WHEN', SUN_WHEN), dropdown('EVENT', SUN_EVENT), value('OFFSET', 'Number')],
    output: 'Boolean',
    style: 'condition_blocks',
    inputsInline: true,
    mutator: 'pb_condition_options',
    tooltip:
      'Compares the time with today\'s sunrise or sunset. A negative shift moves it earlier. "after sunset and before sunrise" means the night. Use ＋ for a second limit.',
  },
  {
    type: 'zone_condition',
    message0: '%1 is in %2',
    args0: [value('ENTITY'), entity('ZONE', 'zone.home', ['zone'])],
    output: 'Boolean',
    style: 'condition_blocks',
    inputsInline: true,
    tooltip: 'True if the person (or device tracker) is in the zone. With several people, all of them must be; with several zones, any of them counts.',
  },
  {
    type: 'ha_template_condition',
    message0: 'template is true %1',
    args0: [multiline('TEMPLATE', "{{ is_state('sun.sun', 'above_horizon') }}")],
    output: 'Boolean',
    style: 'condition_blocks',
    tooltip: 'A Home Assistant template condition: true only when the template renders as "true".',
  },
  {
    type: 'ha_condition_integration',
    message0: 'Home Assistant condition %1 %2 on %3 with options %4',
    args0: [text('NAME', 'light.is_on'), { type: 'input_dummy' }, value('TARGET'), { type: 'input_value', name: 'OPTIONS', check: ['Dict', 'Object'] }],
    output: 'Boolean',
    style: 'condition_blocks',
    inputsInline: false,
    tooltip:
      'Any condition an integration offers, by its name (like light.is_on): what it checks (entities, areas, floors, labels or devices; leave it empty if the condition has none) and its options as a dictionary. The blocks for your own Home Assistant\'s conditions are under Home Assistant.',
  },
];

let registered = false;

/** The settings menu; core blocks use it too, so it is registered before any block. */
export function registerConditionOptions() {
  if (registered) return;
  registered = true;
  Blockly.Extensions.registerMutator('pb_condition_options', mixin, helper);
}

export function registerConditionBlocks() {
  registerConditionOptions();
  Blockly.common.defineBlocksWithJsonArray(CONDITION_BLOCKS);
}
