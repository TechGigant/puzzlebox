//! Keyboard behaviour for dialogs: focus moves in (and back out when closed), Tab stays
//! inside, Escape closes. Only the dialog opened last reacts.

const FOCUSABLE = 'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

const open: HTMLElement[] = [];

/** Svelte action: `<div role="dialog" use:modal={close}>`; `close` null: Escape does nothing. */
export function modal(node: HTMLElement, onclose: (() => void) | null) {
  let close = onclose;
  const before = document.activeElement as HTMLElement | null;
  open.push(node);
  if (!node.hasAttribute('tabindex')) node.setAttribute('tabindex', '-1');
  node.setAttribute('aria-modal', 'true');
  const items = () => [...node.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((e) => e.offsetParent !== null || e === document.activeElement);
  // The first text field, else the dialog itself: a focused button could be pressed by accident.
  queueMicrotask(() => {
    if (node.contains(document.activeElement)) return;
    (items().find((e) => e.matches('input:not([type=checkbox]), select, textarea')) ?? node).focus();
  });

  function key(e: KeyboardEvent) {
    if (open[open.length - 1] !== node) return;
    if (e.key === 'Escape' && close) {
      e.preventDefault();
      e.stopPropagation();
      close();
    } else if (e.key === 'Tab') {
      const all = items();
      if (!all.length) return e.preventDefault();
      const i = all.indexOf(document.activeElement as HTMLElement);
      if (e.shiftKey && (i <= 0 || !node.contains(document.activeElement))) {
        e.preventDefault();
        all[all.length - 1].focus();
      } else if (!e.shiftKey && (i === all.length - 1 || !node.contains(document.activeElement))) {
        e.preventDefault();
        all[0].focus();
      }
    }
  }
  // Capture: before Blockly or the page see the key.
  window.addEventListener('keydown', key, true);
  return {
    update(next: (() => void) | null) {
      close = next;
    },
    destroy() {
      window.removeEventListener('keydown', key, true);
      open.splice(open.indexOf(node), 1);
      if (before?.isConnected) before.focus();
    },
  };
}
