//! Device triggers, conditions and actions (Home Assistant's device automations), grouped by
//! area in the toolbox and loaded when a category is opened.

import * as Blockly from 'blockly/core';
import { getJson } from '../api';
import type { ExtraCategory } from '../toolbox';
import type { HaRegistry } from './field_entity';

interface DeviceParam {
  name: string;
  label: string;
  required: boolean;
  input: 'number' | 'boolean' | 'text' | 'select' | 'duration' | 'any';
  options?: { value: string; label: string }[];
  min?: number;
  max?: number;
  duration_dict?: boolean;
}

interface DeviceAutomation {
  kind: 'trigger' | 'condition' | 'action';
  label: string;
  config: Record<string, unknown>;
  params: DeviceParam[];
  secondary: boolean;
}

interface DeviceState {
  config: Record<string, unknown>;
  label: string;
  params: DeviceParam[];
}

type DeviceBlock = Blockly.BlockSvg & { device: DeviceState | null };

const PREFIX = 'DEVICES:';
const BLOCK_TYPE = { trigger: 'device_trigger', condition: 'device_condition', action: 'device_action' } as const;

function buildInputs(block: DeviceBlock) {
  for (const input of [...block.inputList]) block.removeInput(input.name, true);
  const st = block.device;
  block.appendDummyInput('TITLE').appendField(st?.label || 'device (drag in from Devices)');
  for (const p of st?.params ?? []) {
    const name = `P_${p.name}`;
    switch (p.input) {
      case 'boolean':
        block.appendDummyInput(name).appendField(p.label).appendField(new Blockly.FieldCheckbox('FALSE'), name);
        break;
      case 'select': {
        const opts = (p.options ?? []).map((o) => [o.label || o.value, o.value] as [string, string]);
        if (opts.length) {
          block.appendDummyInput(name).appendField(p.label).appendField(new Blockly.FieldDropdown(opts), name);
          break;
        }
        block.appendDummyInput(name).appendField(p.label).appendField(new Blockly.FieldTextInput(''), name);
        break;
      }
      case 'text':
        block.appendDummyInput(name).appendField(p.label).appendField(new Blockly.FieldTextInput(''), name);
        break;
      default: {
        const label = p.input === 'duration' ? `${p.label.toLowerCase()} (s)` : p.label;
        const input = block.appendValueInput(name).appendField(label);
        input.setAlign(Blockly.inputs.Align.RIGHT);
        if (p.input === 'number' || p.input === 'duration') input.setCheck('Number');
      }
    }
  }
}

function defineDeviceBlock(type: string, kind: DeviceAutomation['kind']) {
  Blockly.Blocks[type] = {
    init(this: DeviceBlock) {
      this.device = null;
      buildInputs(this);
      this.setInputsInline(false);
      if (kind === 'trigger') {
        this.setPreviousStatement(true, 'Trigger');
        this.setNextStatement(true, 'Trigger');
        this.setStyle('trigger_blocks');
      } else if (kind === 'condition') {
        this.setOutput(true, 'Boolean');
        this.setStyle('condition_blocks');
      } else {
        this.setPreviousStatement(true);
        this.setNextStatement(true);
        this.setStyle('action_blocks');
      }
      this.setTooltip(`A device ${kind} from Home Assistant.`);
    },
    saveExtraState(this: DeviceBlock) {
      return this.device;
    },
    loadExtraState(this: DeviceBlock, state: DeviceState | null) {
      this.device = state;
      buildInputs(this);
    },
  };
}

let defined = false;
export function registerDeviceBlocks() {
  if (defined) return;
  defined = true;
  for (const [kind, type] of Object.entries(BLOCK_TYPE)) defineDeviceBlock(type, kind as DeviceAutomation['kind']);
}

/** Devices category with one sub-category per area. */
export function devicesCategory(reg: HaRegistry | null): ExtraCategory | null {
  if (!reg || !reg.devices.length) return null;
  const used = new Set(reg.devices.map((d) => d.area_id ?? ''));
  const areas = reg.areas.filter((a) => used.has(a.area_id)).sort((a, b) => a.name.localeCompare(b.name));
  const subs: ExtraCategory[] = areas.map((a) => ({ name: a.name, style: 'action', blocks: [], custom: PREFIX + a.area_id }));
  if (used.has('')) subs.push({ name: 'No area', style: 'action', blocks: [], custom: PREFIX });
  return { name: 'Devices', style: 'action', blocks: [], subcategories: subs };
}

const automations = new Map<string, DeviceAutomation[]>();
const pending = new Set<string>();

function shadowsFor(params: DeviceParam[]) {
  const inputs: Record<string, unknown> = {};
  for (const p of params) {
    if (p.required && (p.input === 'number' || p.input === 'duration')) {
      inputs[`P_${p.name}`] = { shadow: { type: 'math_number', fields: { NUM: p.min ?? 0 } } };
    }
  }
  return inputs;
}

function flyout(ws: Blockly.WorkspaceSvg, reg: HaRegistry, areaId: string): Blockly.utils.toolbox.FlyoutItemInfo[] {
  const devices = reg.devices
    .filter((d) => (d.area_id ?? '') === areaId && d.name)
    .sort((a, b) => (a.name ?? '').localeCompare(b.name ?? ''));
  const missing = devices.filter((d) => !automations.has(d.id) && !pending.has(d.id));
  if (missing.length) {
    missing.forEach((d) => pending.add(d.id));
    Promise.all(
      missing.map((d) =>
        getJson<DeviceAutomation[]>(`api/ha/devices/${encodeURIComponent(d.id)}/automations`)
          .then((list) => automations.set(d.id, list))
          .catch(() => automations.set(d.id, []))
          .finally(() => pending.delete(d.id)),
      ),
    ).then(() => (ws.getToolbox() as Blockly.Toolbox | null)?.refreshSelection());
  }
  const items: Blockly.utils.toolbox.FlyoutItemInfo[] = [];
  for (const d of devices) {
    const list = automations.get(d.id);
    if (!list) {
      items.push({ kind: 'label', text: `${d.name} (loading…)` } as Blockly.utils.toolbox.FlyoutItemInfo);
      continue;
    }
    if (!list.length) continue;
    items.push({ kind: 'label', text: d.name ?? d.id } as Blockly.utils.toolbox.FlyoutItemInfo);
    const order = { trigger: 0, condition: 1, action: 2 };
    const sorted = [...list].sort((a, b) => order[a.kind] - order[b.kind] || Number(a.secondary) - Number(b.secondary));
    for (const a of sorted) {
      items.push({
        kind: 'block',
        type: BLOCK_TYPE[a.kind],
        extraState: { config: a.config, label: a.label, params: a.params },
        inputs: shadowsFor(a.params),
      } as Blockly.utils.toolbox.FlyoutItemInfo);
    }
  }
  if (!items.length) items.push({ kind: 'label', text: 'No device triggers, conditions or actions' } as Blockly.utils.toolbox.FlyoutItemInfo);
  return items;
}

/** Registers the flyout callbacks for the Devices sub-categories. */
export function registerDeviceCallbacks(ws: Blockly.WorkspaceSvg, reg: HaRegistry | null) {
  if (!reg) return;
  const ids = new Set(reg.devices.map((d) => d.area_id ?? ''));
  for (const id of ids) ws.registerToolboxCategoryCallback(PREFIX + id, () => flyout(ws, reg, id));
}
