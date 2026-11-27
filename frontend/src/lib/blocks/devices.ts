//! Device triggers, conditions and actions (Home Assistant's device automations), grouped by
//! area in the toolbox and loaded when a category is opened.

import * as Blockly from 'blockly/core';
import { getJson } from '../api';
import type { ExtraCategory } from '../toolbox';
import { cachedRegistry, FieldRegistry, loadRegistry, type HaRegistry } from './field_entity';
import { addTriggerSettings } from './trigger_settings';

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

/** The device a block belongs to, shown by name. Not saved itself: the block's config holds
 * the device id. Choosing another device moves the block to that device's matching one. */
class FieldDevice extends FieldRegistry {
  constructor(value: string) {
    super(value, 'device');
  }

  override isSerializable() {
    return false;
  }
}

const kindOf = (type: string) =>
  (Object.entries(BLOCK_TYPE).find(([, t]) => t === type)?.[0] ?? null) as DeviceAutomation['kind'] | null;

const deviceIdOf = (st: DeviceState | null) => (typeof st?.config?.device_id === 'string' ? st.config.device_id : '');

/** The label without the device's name in front ("Hall motion started detecting motion" on the
 * device "Hall motion"), since the device is shown next to it. */
function shownLabel(st: DeviceState | null): string {
  const label = st?.label || 'device (drag in from Devices)';
  const name = cachedRegistry()?.devices.find((d) => d.id === deviceIdOf(st))?.name;
  if (name && label.toLowerCase().startsWith(`${name.toLowerCase()} `)) return label.slice(name.length + 1);
  return label;
}

function buildInputs(block: DeviceBlock) {
  for (const input of [...block.inputList]) block.removeInput(input.name, true);
  const st = block.device;
  const title = block.appendDummyInput('TITLE');
  if (block.type === 'device_trigger') title.appendField('⚡');
  const deviceId = deviceIdOf(st);
  if (deviceId) {
    const field = new FieldDevice(deviceId);
    field.setValidator((v) => {
      if (v && v !== deviceIdOf(block.device)) setTimeout(() => moveToDevice(block, String(v)));
      return v;
    });
    title.appendField(field, 'DEVICE');
  }
  title.appendField(new Blockly.FieldLabel(shownLabel(st)), 'LABEL');
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
        const label = p.input === 'duration' ? `${p.label.toLowerCase()} (seconds)` : p.label;
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
        addTriggerSettings(this);
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
      if (state?.label) this.setTooltip(`${state.label} (a device ${kind} from Home Assistant).`);
      if (this.workspace && !this.workspace.isFlyout) setTimeout(() => checkDevice(this));
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
const fetching = new Map<string, Promise<DeviceAutomation[]>>();

/** A device's triggers, conditions and actions (fetched once). */
function deviceAutomations(deviceId: string): Promise<DeviceAutomation[]> {
  const known = automations.get(deviceId);
  if (known) return Promise.resolve(known);
  let p = fetching.get(deviceId);
  if (!p) {
    p = getJson<DeviceAutomation[]>(`api/ha/devices/${encodeURIComponent(deviceId)}/automations`)
      .then((l) => (automations.set(deviceId, l), l))
      .catch(() => [] as DeviceAutomation[])
      .finally(() => fetching.delete(deviceId));
    fetching.set(deviceId, p);
  }
  return p;
}

/** Keys that say which device and entity an automation is for, not what it does. */
const WHERE = new Set(['device_id', 'entity_id', 'metadata']);

const sameKind = (a: Record<string, unknown>, b: Record<string, unknown>) => {
  const keys = new Set([...Object.keys(a), ...Object.keys(b)].filter((k) => !WHERE.has(k)));
  return [...keys].every((k) => JSON.stringify(a[k]) === JSON.stringify(b[k])) && 'entity_id' in a === 'entity_id' in b;
};

const sameConfig = (a: Record<string, unknown>, b: Record<string, unknown>) =>
  sameKind(a, b) && a.device_id === b.device_id && JSON.stringify(a.entity_id) === JSON.stringify(b.entity_id);

/** Headless blocks (tests, the server-side check) have no warnings. */
const warn = (block: Blockly.Block, text: string | null) =>
  (block as { setWarningText?: (text: string | null, id: string) => void }).setWarningText?.(text, 'device');

const deviceName = (id: string) => cachedRegistry()?.devices.find((d) => d.id === id)?.name ?? id;

/** Shows HA's current label; not an edit, so no undo step or unsaved change. */
function setLabel(block: DeviceBlock, label: string) {
  if (!block.device || block.isDeadOrDying()) return;
  block.device.label = label;
  Blockly.Events.disable();
  try {
    block.getField('LABEL')?.setValue(shownLabel(block.device));
  } finally {
    Blockly.Events.enable();
  }
}

/** Warns about a device that is gone, and gives blocks made elsewhere (imported) HA's own label. */
async function checkDevice(block: DeviceBlock) {
  const deviceId = deviceIdOf(block.device);
  const kind = kindOf(block.type);
  if (!deviceId || !kind || block.isDeadOrDying()) return;
  const reg = await loadRegistry();
  if (block.isDeadOrDying()) return;
  if (reg && !reg.devices.some((d) => d.id === deviceId)) {
    warn(block, 'This device is no longer in Home Assistant.');
    return;
  }
  warn(block, null);
  block.getField('DEVICE')?.forceRerender();
  if (block.device) setLabel(block, block.device.label);
  const list = await deviceAutomations(deviceId);
  const st = block.device;
  const same = st && list.find((a) => a.kind === kind && sameConfig(a.config, st.config));
  if (same && same.label !== st.label) setLabel(block, same.label);
}

/** Points the block at the same trigger/condition/action of another device. */
async function moveToDevice(block: DeviceBlock, deviceId: string) {
  const st = block.device;
  const kind = kindOf(block.type);
  const fromId = deviceIdOf(st);
  if (!st || !kind || !fromId || fromId === deviceId) return;
  const [from, to] = await Promise.all([deviceAutomations(fromId), deviceAutomations(deviceId)]);
  if (block.isDeadOrDying() || block.device !== st) return;
  // A device can have several of the same (two buttons, two lights): take the one in the same place.
  const nth = from.filter((a) => a.kind === kind && sameKind(a.config, st.config)).findIndex((a) => sameConfig(a.config, st.config));
  const matches = to.filter((a) => a.kind === kind && sameKind(a.config, st.config));
  const match = matches[Math.max(nth, 0)] ?? matches[0];
  if (!match) {
    Blockly.Events.disable();
    try {
      block.getField('DEVICE')?.setValue(fromId);
    } finally {
      Blockly.Events.enable();
    }
    warn(block, `${deviceName(deviceId)} has no “${st.label}” ${kind}, so this still uses ${deviceName(fromId)}.`);
    // Only an answer to the choice just made: the block itself is fine.
    setTimeout(() => block.device === st && !block.isDeadOrDying() && warn(block, null), 8000);
    return;
  }
  warn(block, null);
  const before = block.saveExtraState?.() ?? null;
  const sameParams = JSON.stringify(match.params.map((p) => p.name)) === JSON.stringify(st.params.map((p) => p.name));
  const next = { ...(before as object), config: match.config, label: match.label, params: sameParams ? st.params : match.params };
  // Rebuilding the inputs drops field values (settings like the trigger id): put them back.
  const values = new Map<string, unknown>();
  for (const input of block.inputList) {
    for (const f of input.fieldRow) if (f.name && f.isSerializable()) values.set(f.name, f.getValue());
  }
  if (sameParams) {
    // Same settings: keep the inputs (and what is plugged into them).
    block.device = { config: match.config, label: match.label, params: st.params };
    setLabel(block, match.label);
  } else {
    Blockly.Events.disable();
    try {
      block.loadExtraState?.(next);
      for (const [name, v] of values) block.getField(name)?.setValue(v);
    } finally {
      Blockly.Events.enable();
    }
    (block as Partial<Blockly.BlockSvg>).render?.();
  }
  const Change = Blockly.Events.get(Blockly.Events.BLOCK_CHANGE);
  Blockly.Events.fire(new Change(block, 'mutation', null, JSON.stringify(before), JSON.stringify(block.saveExtraState?.() ?? null)));
}

/** Other loaded automations of the same device and kind, as states for the same block type. */
export function deviceAlternatives(block: Blockly.Block): { label: string; extraState: DeviceState }[] {
  const st = (block as DeviceBlock).device;
  const deviceId = st?.config?.device_id;
  const kind = (Object.entries(BLOCK_TYPE).find(([, t]) => t === block.type)?.[0] ?? '') as DeviceAutomation['kind'];
  if (typeof deviceId !== 'string' || !kind) return [];
  return (automations.get(deviceId) ?? [])
    .filter((a) => a.kind === kind && a.label !== st?.label)
    .map((a) => ({ label: a.label, extraState: { config: a.config, label: a.label, params: a.params } }));
}
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
      deviceAutomations(deviceId).finally(() => {
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
