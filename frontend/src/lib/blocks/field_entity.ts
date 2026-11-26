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

/** Resolves which domains the slot this field sits in accepts (set by dynamic blocks). */
export type DomainResolver = (parent: Blockly.Block, inputName: string) => string[];
let resolveDomains: DomainResolver = () => [];
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

  protected override showEditor_(e?: Event): void {
    super.showEditor_(e);
    const input = this.htmlInput_ as HTMLInputElement | null;
    if (!input) return;
    input.addEventListener('input', () => this.renderList(input.value));
    input.addEventListener('keydown', (ev) => this.onKey(ev as KeyboardEvent));
    loadEntities().then(() => this.renderList(input.value));
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
    this.active = 0;

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
      item.className = 'pb-entity-item' + (i === 0 ? ' active' : '');
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

  private allowedDomains(): string[] {
    const block = this.getSourceBlock();
    const parent = block?.getParent();
    if (block && parent) {
      const input = parent.getInputWithBlock(block);
      if (input) {
        const d = resolveDomains(parent, input.name);
        if (d.length) return d;
      }
    }
    return this.domains;
  }

  protected suggestions(q: string): Suggestion[] {
    const domains = this.allowedDomains();
    return cache
      .filter((s) => !domains.length || domains.includes(s.entity_id.split('.')[0]))
      .filter(
        (s) =>
          !q || s.entity_id.toLowerCase().includes(q) || (s.attributes.friendly_name ?? '').toLowerCase().includes(q),
      )
      .map((s) => ({ value: s.entity_id, label: s.attributes.friendly_name ?? s.entity_id, meta: `${s.entity_id} · ${s.state}` }));
  }

  protected emptyText() {
    return cache.length ? 'No matching entities' : 'Loading entities…';
  }
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
let resolveTarget: TargetResolver = () => [];
export function setTargetResolver(r: TargetResolver) {
  resolveTarget = r;
}

/**
 * A state or attribute value of the block's target entity, e.g. an event entity's event types
 * or a climate entity's HVAC modes. Several values can be separated with commas.
 */
export class FieldStateValue extends FieldSuggest {
  attribute: string | null = null;

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
    // Suggest for the part after the last comma, keeping earlier values.
    const raw = (this.htmlInput_ as HTMLInputElement | null)?.value ?? '';
    const head = raw.includes(',') ? raw.slice(0, raw.lastIndexOf(',') + 1) + ' ' : '';
    const part = q.includes(',') ? q.slice(q.lastIndexOf(',') + 1).trim() : q;
    return this.candidates()
      .filter((v) => !part || v.toLowerCase().includes(part))
      .map((v) => ({ value: head + v, label: v }));
  }

  protected emptyText() {
    const block = this.getSourceBlock();
    return block && resolveTarget(block).length ? 'No known values; type one' : 'Choose an entity first';
  }
}

let registered = false;
export function registerFieldEntity() {
  if (registered) return;
  registered = true;
  Blockly.fieldRegistry.register('field_entity', FieldEntity);
}
