//! The id and name (alias) every Home Assistant trigger block can be given, added from its
//! "＋ option" menu (generated blocks: their "＋ more settings" menu). The id is what
//! "triggered by" and `trigger.id` see; several triggers may share one.

import * as Blockly from 'blockly/core';

interface Setting {
  name: string;
  /** Menu text for adding it. */
  label: string;
  /** Text before its box. */
  row: string;
}

const SETTINGS: Setting[] = [
  { name: 'TRIGGER_ID', label: 'trigger id', row: 'with trigger id' },
  { name: 'TRIGGER_ALIAS', label: 'name', row: 'named' },
];

const MENU_INPUT = 'TRIGGER_MENU';
const MENU = '__menu__';

type SettingsBlock = Blockly.Block & { triggerSettings: string[]; triggerMenuBefore?: string };
type Extra = Record<string, unknown> | null;

/** Block types that have these settings. */
const types = new Set<string>();

export function hasTriggerSettings(type: string): boolean {
  return types.has(type);
}

/** A menu, not a setting: the chosen settings are saved in the extra state. */
class MenuField extends Blockly.FieldDropdown {
  override isSerializable() {
    return false;
  }
}

/** Where rows go: before the block's own menu, or before ours at the end. */
function anchor(block: SettingsBlock): string | null {
  if (block.triggerMenuBefore && block.getInput(block.triggerMenuBefore)) return block.triggerMenuBefore;
  return block.getInput(MENU_INPUT) ? MENU_INPUT : null;
}

function addRow(block: SettingsBlock, name: string) {
  const s = SETTINGS.find((x) => x.name === name);
  if (!s || block.getInput(name)) return;
  if (!block.triggerSettings.includes(name)) block.triggerSettings.push(name);
  // Ends the line before, so each setting gets its own line.
  block.appendEndRowInput(`${name}__row`);
  block.appendDummyInput(name).appendField(s.row).appendField(new Blockly.FieldTextInput(''), name);
  const at = anchor(block);
  if (at) {
    block.moveInputBefore(`${name}__row`, at);
    block.moveInputBefore(name, at);
  }
}

function removeRow(block: SettingsBlock, name: string) {
  block.triggerSettings = block.triggerSettings.filter((n) => n !== name);
  if (block.getInput(name)) block.removeInput(name, true);
  if (block.getInput(`${name}__row`)) block.removeInput(`${name}__row`, true);
}

/** Menu entries for adding or removing the settings ([label, "+NAME" or "-NAME"]). */
export function triggerSettingItems(block: Blockly.Block): [string, string][] {
  const b = block as SettingsBlock;
  return SETTINGS.map((s) => (b.triggerSettings?.includes(s.name) ? [`− remove: ${s.label}`, `-${s.name}`] : [`＋ ${s.label}`, `+${s.name}`]));
}

/** Handles a menu choice from `triggerSettingItems`; false if it wasn't one. */
export function chooseTriggerSetting(block: Blockly.Block, choice: string): boolean {
  const b = block as SettingsBlock;
  const name = choice.slice(1);
  if (!SETTINGS.some((s) => s.name === name) || !'+-'.includes(choice[0])) return false;
  const before = JSON.stringify(block.saveExtraState?.() ?? null);
  if (choice[0] === '+') addRow(b, name);
  else removeRow(b, name);
  const after = JSON.stringify(block.saveExtraState?.() ?? null);
  if (before !== after) {
    const Change = Blockly.Events.get(Blockly.Events.BLOCK_CHANGE);
    Blockly.Events.fire(new Change(block, 'mutation', null, before, after));
  }
  if (block.rendered) (block as Blockly.BlockSvg).render();
  return true;
}

function ensureMenu(block: SettingsBlock) {
  if (block.triggerMenuBefore || block.getInput(MENU_INPUT)) return;
  const field = new MenuField(
    () => [['＋ option', MENU], ...triggerSettingItems(block)],
    (v) => {
      if (v && v !== MENU) setTimeout(() => chooseTriggerSetting(block, v));
      return null;
    },
  );
  block.appendDummyInput(MENU_INPUT).appendField(field, `${MENU_INPUT}_FIELD`);
}

/** Shows the rows in `wanted` (and only those), e.g. after the block rebuilt its inputs. */
function apply(block: SettingsBlock, wanted: string[]) {
  ensureMenu(block);
  for (const name of SETTINGS.map((s) => s.name)) {
    if (!wanted.includes(name)) removeRow(block, name);
  }
  block.triggerSettings = block.triggerSettings.filter((n) => wanted.includes(n));
  for (const name of wanted) addRow(block, name);
}

/**
 * Gives a trigger block the settings: call at the end of its `init`. `menuBefore` names the
 * input of a menu the block already has (the settings then go into that menu and their rows
 * before it); otherwise the block gets its own "＋ option" menu.
 */
export function addTriggerSettings(block: Blockly.Block, menuBefore?: string) {
  const b = block as SettingsBlock;
  types.add(block.type);
  b.triggerSettings = [];
  b.triggerMenuBefore = menuBefore;
  ensureMenu(b);
  const save = block.saveExtraState?.bind(block);
  const load = block.loadExtraState?.bind(block);
  block.saveExtraState = (full?: boolean) => {
    let base = (save?.(full) ?? null) as Extra;
    if (base && typeof base === 'object' && 'triggerSettings' in base) {
      const { triggerSettings: _, ...rest } = base;
      base = Object.keys(rest).length ? rest : null;
    }
    if (!b.triggerSettings.length) return base;
    return { ...(base ?? {}), triggerSettings: [...b.triggerSettings] };
  };
  block.loadExtraState = (state: Extra) => {
    load?.(state);
    const wanted = Array.isArray(state?.triggerSettings) ? (state.triggerSettings as string[]) : [];
    apply(b, wanted);
  };
}

/** Trigger blocks (JSON definitions) that compile to an HA trigger config; the raw one has the
 * YAML for that and engine triggers report their block id. */
export function haTriggerTypes(blocks: Record<string, unknown>[]): string[] {
  return blocks
    .filter((b) => b.previousStatement === 'Trigger')
    .map((b) => String(b.type))
    .filter((t) => !t.startsWith('engine_') && t !== 'ha_trigger_raw');
}

/** Adds the settings to these block types (defined from JSON); call after defining them. */
export function installTriggerSettings(blockTypes: string[]) {
  for (const type of blockTypes) {
    const def = Blockly.Blocks[type] as { init: (this: Blockly.Block) => void; pbTriggerSettings?: boolean } | undefined;
    if (!def || def.pbTriggerSettings) continue;
    const init = def.init;
    def.init = function (this: Blockly.Block) {
      init.call(this);
      addTriggerSettings(this);
    };
    def.pbTriggerSettings = true;
  }
}
