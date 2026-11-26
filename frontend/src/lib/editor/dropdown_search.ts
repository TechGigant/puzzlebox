//! Type-to-filter in long dropdown menus (services, options, units…).

import * as Blockly from 'blockly/core';

const MIN_OPTIONS = 9;

type Opt = [string | Blockly.ImageProperties | HTMLElement, string] | string;
const labelOf = (o: Opt) => {
  if (!Array.isArray(o)) return '';
  const l = o[0];
  return (typeof l === 'string' ? l : l instanceof HTMLElement ? l.textContent ?? '' : (l.alt ?? '')).toLowerCase();
};

let installed = false;
export function installDropdownSearch() {
  if (installed) return;
  installed = true;
  const proto = Blockly.FieldDropdown.prototype as unknown as { showEditor_: (e?: MouseEvent) => void };
  const original = proto.showEditor_;
  proto.showEditor_ = function (this: Blockly.FieldDropdown, e?: MouseEvent) {
    original.call(this, e);
    const options = this.getOptions(false) as Opt[];
    if (options.filter(Array.isArray).length < MIN_OPTIONS) return;
    const content = Blockly.DropDownDiv.getContentDiv();
    const menuEl = content.querySelector<HTMLElement>('.blocklyMenu');
    if (!menuEl) return;
    const items = [...menuEl.querySelectorAll<HTMLElement>('.blocklyMenuItem')];
    const real = options.filter(Array.isArray) as [unknown, string][];
    const input = document.createElement('input');
    input.className = 'pb-dropdown-search';
    input.placeholder = 'Filter…';
    input.setAttribute('aria-label', 'Filter options');
    content.prepend(input);
    const visible = () => items.filter((el) => el.style.display !== 'none');
    input.addEventListener('input', () => {
      const q = input.value.trim().toLowerCase();
      items.forEach((el, i) => {
        const match = !q || labelOf(real[i] as Opt).includes(q) || String(real[i]?.[1] ?? '').toLowerCase().includes(q);
        el.style.display = match ? '' : 'none';
      });
    });
    input.addEventListener('keydown', (ev) => {
      if (ev.key === 'Enter') {
        ev.preventDefault();
        const first = visible()[0];
        const i = first ? items.indexOf(first) : -1;
        if (i >= 0) {
          this.setValue(real[i][1]);
          Blockly.DropDownDiv.hideIfOwner(this, true);
        }
      } else if (ev.key === 'Escape') {
        Blockly.DropDownDiv.hideIfOwner(this, true);
      }
      ev.stopPropagation();
    });
    // The menu grabs focus when it opens; take it once it has.
    setTimeout(() => input.focus(), 0);
  };
}
