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

/** Devices that can have device automations: named, and with at least one entity. */
function usableDevices(reg: HaRegistry) {
  const withEntities = new Set(reg.entities.map((e) => e.device_id).filter(Boolean));
  return reg.devices
    .filter((d) => d.name && withEntities.has(d.id))
    .sort((a, b) => (a.name ?? '').localeCompare(b.name ?? ''));
}

/** Devices → one sub-category per area → one per device (its flyout loads when opened). */
export function devicesCategory(reg: HaRegistry | null): ExtraCategory | null {
  if (!reg) return null;
  const devices = usableDevices(reg);
  if (!devices.length) return null;
  const deviceCat = (d: (typeof devices)[number]): ExtraCategory => ({
    name: d.name ?? d.id,
    style: 'action',
    blocks: [],
    custom: PREFIX + d.id,
  });
  const used = new Set(devices.map((d) => d.area_id ?? ''));
  const areas = reg.areas.filter((a) => used.has(a.area_id)).sort((a, b) => a.name.localeCompare(b.name));
  const subs: ExtraCategory[] = areas.map((a) => ({
    name: a.name,
    style: 'action',
    blocks: [],
    subcategories: devices.filter((d) => d.area_id === a.area_id).map(deviceCat),
  }));
  const loose = devices.filter((d) => !d.area_id);
  if (loose.length) subs.push({ name: 'No area', style: 'action', blocks: [], subcategories: loose.map(deviceCat) });
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

type FlyoutItem = Blockly.utils.toolbox.FlyoutItemInfo;
const label = (text: string) => ({ kind: 'label', text }) as FlyoutItem;
const KIND_LABEL = { trigger: 'Triggers', condition: 'Conditions', action: 'Actions' } as const;

function flyout(ws: Blockly.WorkspaceSvg, deviceId: string): FlyoutItem[] {
  const list = automations.get(deviceId);
  if (!list) {
    if (!pending.has(deviceId)) {
      pending.add(deviceId);
      getJson<DeviceAutomation[]>(`api/ha/devices/${encodeURIComponent(deviceId)}/automations`)
        .then((l) => automations.set(deviceId, l))
        .catch(() => automations.set(deviceId, []))
        .finally(() => {
          pending.delete(deviceId);
          (ws.getToolbox() as Blockly.Toolbox | null)?.refreshSelection();
        });
    }
    return [label('Loading…')];
  }
  if (!list.length) return [label('This device has no device triggers, conditions or actions')];
  const items: FlyoutItem[] = [];
  for (const kind of ['trigger', 'condition', 'action'] as const) {
    const of = list.filter((a) => a.kind === kind).sort((a, b) => Number(a.secondary) - Number(b.secondary));
    if (!of.length) continue;
    items.push(label(KIND_LABEL[kind]));
    for (const a of of) {
      items.push({
        kind: 'block',
        type: BLOCK_TYPE[a.kind],
        extraState: { config: a.config, label: a.label, params: a.params },
        inputs: shadowsFor(a.params),
      } as FlyoutItem);
    }
  }
  return items;
}

/** Registers the flyout callbacks for the per-device categories. */
export function registerDeviceCallbacks(ws: Blockly.WorkspaceSvg, reg: HaRegistry | null) {
  if (!reg) return;
  for (const d of usableDevices(reg)) ws.registerToolboxCategoryCallback(PREFIX + d.id, () => flyout(ws, d.id));
}
