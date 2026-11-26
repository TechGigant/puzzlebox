import * as Blockly from 'blockly/core';
import { getJson } from '../api';

export interface EntityState {
  entity_id: string;
  state: string;
  attributes: Record<string, unknown> & { friendly_name?: string };
}

let cache: EntityState[] = [];
let fetchedAt = 0;
let inflight: Promise<void> | null = null;

export function loadEntities(force = false): Promise<void> {
  if (!force && Date.now() - fetchedAt < 30_000) return Promise.resolve();
  inflight ??= getJson<EntityState[]>('api/ha/states')
    .then((list) => {
      cache = list.sort((a, b) => a.entity_id.localeCompare(b.entity_id));
      fetchedAt = Date.now();
    })
    .catch(() => {})
    .finally(() => (inflight = null));
  return inflight;
}

export function entityState(id: string): EntityState | undefined {
  return cache.find((s) => s.entity_id === id);
}

export interface EntityFilter {
  domains: string[];
  deviceClasses?: string[];
}

/** Resolves which entities the slot this field sits in accepts (set by dynamic blocks). */
export type DomainResolver = (parent: Blockly.Block, inputName: string) => EntityFilter;
let resolveDomains: DomainResolver = () => ({ domains: [] });
export function setDomainResolver(r: DomainResolver) {
  resolveDomains = r;
}

export interface Suggestion {
  value: string;
  label: string;
  meta?: string;
}

/**
 * Text field with a searchable suggestion list. Free text always stays allowed, so values
 * that aren't suggested (templates, entities not loaded yet) never break a saved program.
 */
export abstract class FieldSuggest extends Blockly.FieldTextInput {
  private list: HTMLDivElement | null = null;
  private active = 0;
  private matches: Suggestion[] = [];

  protected abstract suggestions(query: string): Suggestion[];
  protected abstract emptyText(): string;
  protected load(): Promise<void> {
    return loadEntities();
  }

  /** Suggestions for the part after the last comma, keeping the values before it. */
  protected commaList(q: string, candidates: Suggestion[]): Suggestion[] {
    const raw = (this.htmlInput_ as HTMLInputElement | null)?.value ?? '';
    const head = raw.includes(',') ? raw.slice(0, raw.lastIndexOf(',') + 1) + ' ' : '';
    const part = q.includes(',') ? q.slice(q.lastIndexOf(',') + 1).trim() : q;
    return candidates
      .filter((c) => !part || c.label.toLowerCase().includes(part) || c.value.toLowerCase().includes(part))
      .map((c) => ({ ...c, value: head + c.value }));
  }

  protected override showEditor_(e?: Event): void {
    super.showEditor_(e);
    const input = this.htmlInput_ as HTMLInputElement | null;
    if (!input) return;
    input.addEventListener('input', () => this.renderList(input.value));
    input.addEventListener('keydown', (ev) => this.onKey(ev as KeyboardEvent));
    // Show every option at first, like a dropdown; typing narrows the list.
    this.load().then(() => this.renderList(''));
  }

  private onKey(ev: KeyboardEvent) {
    if (!this.list || !this.matches.length) return;
    if (ev.key === 'ArrowDown' || ev.key === 'ArrowUp') {
      ev.preventDefault();
      const d = ev.key === 'ArrowDown' ? 1 : -1;
      this.active = (this.active + d + this.matches.length) % this.matches.length;
      this.highlight();
    } else if (ev.key === 'Enter' && this.matches[this.active]) {
      ev.preventDefault();
      this.pick(this.matches[this.active].value);
    }
  }

  private highlight() {
    this.list?.querySelectorAll('.pb-entity-item').forEach((el, i) => {
      el.classList.toggle('active', i === this.active);
      if (i === this.active) (el as HTMLElement).scrollIntoView({ block: 'nearest' });
    });
  }

  private pick(value: string) {
    this.setValue(value);
    Blockly.WidgetDiv.hide();
    Blockly.DropDownDiv.hideIfOwner(this);
  }

  private renderList(query: string) {
    this.matches = this.suggestions(query.trim().toLowerCase()).slice(0, 80);
    // With nothing typed yet, start at the current value.
    const current = query ? -1 : this.matches.findIndex((m) => m.value === this.getValue());
    this.active = Math.max(current, 0);

    const content = Blockly.DropDownDiv.getContentDiv();
    content.replaceChildren();
    const list = document.createElement('div');
    list.className = 'pb-entity-list';
    if (!this.matches.length) {
      const empty = document.createElement('div');
      empty.className = 'pb-entity-empty';
      empty.textContent = this.emptyText();
      list.append(empty);
    }
    this.matches.forEach((s, i) => {
      const item = document.createElement('div');
      item.className = 'pb-entity-item' + (i === this.active ? ' active' : '');
      const name = document.createElement('div');
      name.className = 'pb-entity-name';
      name.textContent = s.label;
      item.append(name);
      if (s.meta) {
        const meta = document.createElement('div');
        meta.className = 'pb-entity-meta';
        meta.textContent = s.meta;
        item.append(meta);
      }
      // mousedown keeps focus in the text input (click would blur and close the editor first).
      item.addEventListener('mousedown', (ev) => {
        ev.preventDefault();
        this.pick(s.value);
      });
      list.append(item);
    });
    content.append(list);
    this.list = list;
    Blockly.DropDownDiv.setColour('var(--pb-card)', 'var(--pb-divider)');
    // The text editor (WidgetDiv) already holds ephemeral focus; the list must not take it too.
    Blockly.DropDownDiv.showPositionedByField(this, () => (this.list = null), undefined, false);
    this.highlight();
  }
}

/** Entity picker, filtered to the domains the surrounding slot accepts. */
export class FieldEntity extends FieldSuggest {
  private domains: string[] = [];

  static fromJson(options: Blockly.FieldTextInputFromJsonConfig & { domains?: string[] }) {
    const f = new FieldEntity(options.text ?? '');
    f.domains = options.domains ?? [];
    return f;
  }

  private filter(): EntityFilter {
    const block = this.getSourceBlock();
    const parent = block?.getParent();
    if (block && parent) {
      const input = parent.getInputWithBlock(block);
      if (input) {
        const f = resolveDomains(parent, input.name);
        if (f.domains.length || f.deviceClasses?.length) return f;
      }
    }
    return { domains: this.domains };
  }

  protected suggestions(q: string): Suggestion[] {
    const { domains, deviceClasses = [] } = this.filter();
    const all = cache
      .filter((s) => !domains.length || domains.includes(s.entity_id.split('.')[0]))
      .filter((s) => !deviceClasses.length || deviceClasses.includes(String(s.attributes.device_class ?? '')))
      .map((s) => ({ value: s.entity_id, label: s.attributes.friendly_name ?? s.entity_id, meta: `${s.entity_id} · ${s.state}` }));
    // Several entities can be listed with commas.
    const raw = (this.htmlInput_ as HTMLInputElement | null)?.value ?? '';
    if (raw.includes(',')) return this.commaList(q, all);
    return all
      .filter((s) => !q || s.value.toLowerCase().includes(q) || s.label.toLowerCase().includes(q));
  }

  protected emptyText() {
    return cache.length ? 'No matching entities' : 'Loading entities…';
  }

  protected override getText_(): string | null {
    // Show names ("Kitchen light") instead of ids; the editor still edits the id.
    const v = String(this.getValue() ?? '');
    if (!v) return 'choose…';
    if (v.includes('{')) return null;
    const names = v.split(',').map((id) => entityState(id.trim())?.attributes.friendly_name ?? id.trim());
    return names.join(', ');
  }
}

/** Redraws entity and registry fields so they show names once the lists have loaded. */
export function refreshNameFields(ws: Blockly.Workspace) {
  for (const b of ws.getAllBlocks(false)) {
    for (const input of b.inputList) {
      for (const f of input.fieldRow) {
        if (f instanceof FieldEntity || f instanceof FieldRegistry) f.forceRerender();
      }
    }
  }
}

/** Entities loaded from Home Assistant (empty until the first load finishes). */
export function knownEntities(): EntityState[] {
  return cache;
}

// Tables below follow Home Assistant's frontend (src/common/entity/get_states.ts, Apache-2.0).

/** States an entity of each domain can have, when the entity doesn't list them itself. */
const FIXED_DOMAIN_STATES: Record<string, string[]> = {
  alarm_control_panel: ['armed_away', 'armed_custom_bypass', 'armed_home', 'armed_night', 'armed_vacation', 'arming', 'disarmed', 'disarming', 'pending', 'triggered'],
  alert: ['on', 'off', 'idle'],
  assist_satellite: ['idle', 'listening', 'responding', 'processing'],
  automation: ['on', 'off'],
  binary_sensor: ['on', 'off'],
  calendar: ['on', 'off'],
  camera: ['idle', 'recording', 'streaming'],
  cover: ['closed', 'closing', 'open', 'opening'],
  device_tracker: ['home', 'not_home'],
  fan: ['on', 'off'],
  humidifier: ['on', 'off'],
  input_boolean: ['on', 'off'],
  lawn_mower: ['error', 'paused', 'mowing', 'returning', 'docked', 'idle'],
  light: ['on', 'off'],
  lock: ['jammed', 'locked', 'locking', 'unlocked', 'unlocking', 'opening', 'open'],
  media_player: ['off', 'on', 'idle', 'playing', 'paused', 'standby', 'buffering'],
  person: ['home', 'not_home'],
  plant: ['ok', 'problem'],
  remote: ['on', 'off'],
  schedule: ['on', 'off'],
  script: ['on', 'off'],
  siren: ['on', 'off'],
  sun: ['above_horizon', 'below_horizon'],
  switch: ['on', 'off'],
  timer: ['active', 'idle', 'paused'],
  update: ['on', 'off'],
  vacuum: ['cleaning', 'docked', 'error', 'idle', 'paused', 'returning'],
  valve: ['closed', 'closing', 'open', 'opening'],
  weather: ['clear-night', 'cloudy', 'exceptional', 'fog', 'hail', 'lightning-rainy', 'lightning', 'partlycloudy', 'pouring', 'rainy', 'snowy-rainy', 'snowy', 'sunny', 'windy-variant', 'windy'],
};

/** Per domain: value attribute (`_` = the state) → attribute listing its options. */
const DOMAIN_OPTIONS_ATTRIBUTES: Record<string, Record<string, string>> = {
  climate: { _: 'hvac_modes', fan_mode: 'fan_modes', preset_mode: 'preset_modes', swing_mode: 'swing_modes', swing_horizontal_mode: 'swing_horizontal_modes' },
  cover: { speed: 'supported_speeds' },
  event: { event_type: 'event_types' },
  fan: { preset_mode: 'preset_modes' },
  humidifier: { mode: 'available_modes' },
  input_select: { _: 'options' },
  select: { _: 'options' },
  light: { effect: 'effect_list', color_mode: 'supported_color_modes' },
  media_player: { sound_mode: 'sound_mode_list', source: 'source_list' },
  remote: { current_activity: 'activity_list' },
  sensor: { _: 'options' },
  vacuum: { fan_speed: 'fan_speed_list' },
  water_heater: { _: 'operation_list', operation_mode: 'operation_list' },
};

/** Resolves the entity ids in a block's target slot (set by dynamic blocks). */
export type TargetResolver = (block: Blockly.Block) => string[];
const split = (raw: string) =>
  raw
    .split(',')
    .map((x) => x.trim())
    .filter(Boolean);
/** Entities a block is about: its TARGET or ENTITY socket (entity block), or ENTITY field. */
export function blockEntities(block: Blockly.Block): string[] {
  for (const name of ['TARGET', 'ENTITY']) {
    const t = block.getInputTargetBlock(name);
    if (t?.type === 'ha_entity') return split(String(t.getFieldValue('ENTITY') ?? ''));
  }
  const f = block.getField('ENTITY');
  return f ? split(String(f.getValue() ?? '')) : [];
}
let resolveTarget: TargetResolver = blockEntities;
export function setTargetResolver(r: TargetResolver) {
  resolveTarget = r;
}

/**
 * A state or attribute value of the block's target entity, e.g. an event entity's event types
 * or a climate entity's HVAC modes. Several values can be separated with commas.
 */
export class FieldStateValue extends FieldSuggest {
  attribute: string | null = null;

  static fromJson(options: Blockly.FieldTextInputFromJsonConfig) {
    return new FieldStateValue(options.text ?? '');
  }

  private candidates(): string[] {
    const block = this.getSourceBlock();
    const ids = block ? resolveTarget(block) : [];
    const values = new Set<string>();
    for (const id of ids) {
      const s = entityState(id);
      const domain = id.split('.')[0];
      const listAttr = DOMAIN_OPTIONS_ATTRIBUTES[domain]?.[this.attribute ?? '_'];
      const listed = listAttr ? s?.attributes[listAttr] : undefined;
      if (Array.isArray(listed)) listed.forEach((v) => values.add(String(v)));
      if (!this.attribute) {
        (FIXED_DOMAIN_STATES[domain] ?? []).forEach((v) => values.add(v));
        if (s && !['unknown', 'unavailable'].includes(s.state)) values.add(s.state);
      } else if (s && s.attributes[this.attribute] != null) {
        values.add(String(s.attributes[this.attribute]));
      }
    }
    return [...values];
  }

  protected suggestions(q: string): Suggestion[] {
    return this.commaList(q, this.candidates().map((v) => ({ value: v, label: v })));
  }

  protected emptyText() {
    const block = this.getSourceBlock();
    return block && resolveTarget(block).length ? 'No known values; type one' : 'Choose an entity first';
  }
}

/** Free text suggesting fixed options; several values can be separated with commas. */
export class FieldOptions extends FieldSuggest {
  constructor(
    value: string,
    private options: Suggestion[],
  ) {
    super(value);
  }

  protected override load() {
    return Promise.resolve();
  }

  protected suggestions(q: string): Suggestion[] {
    return this.commaList(q, this.options);
  }

  protected emptyText() {
    return 'Type a value';
  }
}

export interface HaRegistry {
  areas: { area_id: string; name: string; floor_id?: string | null }[];
  floors: { floor_id: string; name: string }[];
  labels: { label_id: string; name: string }[];
  devices: { id: string; name: string | null; area_id: string | null; manufacturer?: string | null; model?: string | null }[];
  entities: { entity_id: string; device_id: string | null; area_id: string | null }[];
}

let registryCache: HaRegistry | null = null;
let registryAt = 0;
let registryInflight: Promise<HaRegistry | null> | null = null;

export function loadRegistry(force = false): Promise<HaRegistry | null> {
  if (!force && registryCache && Date.now() - registryAt < 60_000) return Promise.resolve(registryCache);
  registryInflight ??= getJson<HaRegistry>('api/ha/registry')
    .then((r) => {
      registryCache = r;
      registryAt = Date.now();
      return r;
    })
    .catch(() => registryCache)
    .finally(() => (registryInflight = null));
  return registryInflight;
}

export function registrySuggestions(r: HaRegistry | null, kind: string): Suggestion[] {
  if (!r) return [];
  const areaName = (id: string | null | undefined) => r.areas.find((a) => a.area_id === id)?.name;
  switch (kind) {
    case 'area':
      return r.areas.map((a) => ({ value: a.area_id, label: a.name, meta: a.area_id }));
    case 'floor':
      return r.floors.map((f) => ({ value: f.floor_id, label: f.name, meta: f.floor_id }));
    case 'label':
      return r.labels.map((l) => ({ value: l.label_id, label: l.name, meta: l.label_id }));
    default:
      return r.devices
        .filter((d) => d.name)
        .map((d) => ({ value: d.id, label: d.name ?? d.id, meta: [areaName(d.area_id), d.model].filter(Boolean).join(' · ') }));
  }
}

/** Area, floor, label or device picker (the value is the registry id). */
export class FieldRegistry extends FieldSuggest {
  constructor(
    value: string,
    private kind: string,
  ) {
    super(value);
  }

  static fromJson(options: Blockly.FieldTextInputFromJsonConfig & { kind?: string }) {
    return new FieldRegistry(options.text ?? '', options.kind ?? 'area');
  }

  protected override load() {
    return loadRegistry().then(() => undefined);
  }

  protected suggestions(q: string): Suggestion[] {
    return this.commaList(q, registrySuggestions(registryCache, this.currentKind()));
  }

  protected emptyText() {
    return registryCache ? `No ${this.currentKind()}s found` : 'Loading…';
  }

  /** Blocks with a KIND dropdown (area/floor/…) pick from that kind. */
  private currentKind(): string {
    const k = this.getSourceBlock()?.getField('KIND')?.getValue();
    return typeof k === 'string' ? k : this.kind;
  }

  protected override getText_(): string | null {
    // Show names instead of ids.
    const v = String(this.getValue() ?? '');
    if (!registryCache || !v) return null;
    const names = registrySuggestions(registryCache, this.currentKind());
    return v
      .split(',')
      .map((id) => names.find((n) => n.value === id.trim())?.label ?? id.trim())
      .join(', ');
  }
}

/** Attribute names of the block's entity (brightness, friendly_name, …). */
export class FieldAttribute extends FieldSuggest {
  static fromJson(options: Blockly.FieldTextInputFromJsonConfig) {
    return new FieldAttribute(options.text ?? '');
  }

  protected suggestions(q: string): Suggestion[] {
    const block = this.getSourceBlock();
    const names = new Set<string>();
    for (const id of block ? blockEntities(block) : []) {
      for (const k of Object.keys(entityState(id)?.attributes ?? {})) names.add(k);
    }
    return [...names]
      .filter((n) => !q || n.includes(q))
      .map((n) => {
        const ids = block ? blockEntities(block) : [];
        const v = ids.length ? entityState(ids[0])?.attributes[n] : undefined;
        return { value: n, label: n.replaceAll('_', ' '), meta: v === undefined ? undefined : String(JSON.stringify(v)).slice(0, 40) };
      });
  }

  protected emptyText() {
    return 'Choose an entity first';
  }
}

/** A time of day ("07:30") or an input_datetime entity; "7", "730" and "7.30" are tidied up. */
export class FieldTime extends Blockly.FieldTextInput {
  static fromJson(options: Blockly.FieldTextInputFromJsonConfig) {
    return new FieldTime(options.text ?? '');
  }

  protected override doClassValidation_(v?: string): string | null {
    const s = String(v ?? '').trim();
    if (!s || /^[a-z_]+\.[a-z0-9_]+$/.test(s)) return s;
    const m = s.match(/^(\d{1,2})(?:[:.h]?(\d{2}))?(?:[:.](\d{2}))?$/);
    if (!m) return null;
    const [h, min, sec] = [Number(m[1]), Number(m[2] ?? 0), m[3] === undefined ? null : Number(m[3])];
    if (h > 23 || min > 59 || (sec ?? 0) > 59) return null;
    const pad = (n: number) => String(n).padStart(2, '0');
    return `${pad(h)}:${pad(min)}${sec === null ? '' : ':' + pad(sec)}`;
  }
}

let registered = false;
export function registerFieldEntity() {
  if (registered) return;
  registered = true;
  Blockly.fieldRegistry.register('field_entity', FieldEntity);
  Blockly.fieldRegistry.register('field_registry', FieldRegistry);
  Blockly.fieldRegistry.register('field_state_value', FieldStateValue);
  Blockly.fieldRegistry.register('field_attribute', FieldAttribute);
  Blockly.fieldRegistry.register('field_time', FieldTime);
}
