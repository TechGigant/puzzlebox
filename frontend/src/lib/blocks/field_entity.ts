import * as Blockly from 'blockly/core';
import { getJson } from '../api';

interface EntityState {
  entity_id: string;
  state: string;
  attributes: { friendly_name?: string };
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

/** Resolves which domains the slot this field sits in accepts (set by dynamic blocks). */
export type DomainResolver = (parent: Blockly.Block, inputName: string) => string[];
let resolveDomains: DomainResolver = () => [];
export function setDomainResolver(r: DomainResolver) {
  resolveDomains = r;
}

/** Text field with a searchable entity list; free text (templates, ids not loaded yet) stays allowed. */
export class FieldEntity extends Blockly.FieldTextInput {
  private list: HTMLDivElement | null = null;
  private active = 0;
  private matches: EntityState[] = [];
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
      this.pick(this.matches[this.active].entity_id);
    }
  }

  private highlight() {
    this.list?.querySelectorAll('.pb-entity-item').forEach((el, i) => {
      el.classList.toggle('active', i === this.active);
      if (i === this.active) (el as HTMLElement).scrollIntoView({ block: 'nearest' });
    });
  }

  private pick(id: string) {
    this.setValue(id);
    Blockly.WidgetDiv.hide();
    Blockly.DropDownDiv.hideIfOwner(this);
  }

  private renderList(query: string) {
    const domains = this.allowedDomains();
    const q = query.trim().toLowerCase();
    this.matches = cache
      .filter((s) => !domains.length || domains.includes(s.entity_id.split('.')[0]))
      .filter(
        (s) =>
          !q ||
          s.entity_id.toLowerCase().includes(q) ||
          (s.attributes.friendly_name ?? '').toLowerCase().includes(q),
      )
      .slice(0, 80);
    this.active = 0;

    const content = Blockly.DropDownDiv.getContentDiv();
    content.replaceChildren();
    const list = document.createElement('div');
    list.className = 'pb-entity-list';
    if (!this.matches.length) {
      const empty = document.createElement('div');
      empty.className = 'pb-entity-empty';
      empty.textContent = cache.length ? 'No matching entities' : 'Loading entities…';
      list.append(empty);
    }
    this.matches.forEach((s, i) => {
      const item = document.createElement('div');
      item.className = 'pb-entity-item' + (i === 0 ? ' active' : '');
      const name = document.createElement('div');
      name.className = 'pb-entity-name';
      name.textContent = s.attributes.friendly_name ?? s.entity_id;
      const meta = document.createElement('div');
      meta.className = 'pb-entity-meta';
      meta.textContent = `${s.entity_id} · ${s.state}`;
      item.append(name, meta);
      // mousedown keeps focus in the text input (click would blur and close the editor first).
      item.addEventListener('mousedown', (ev) => {
        ev.preventDefault();
        this.pick(s.entity_id);
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

let registered = false;
export function registerFieldEntity() {
  if (registered) return;
  registered = true;
  Blockly.fieldRegistry.register('field_entity', FieldEntity);
}
