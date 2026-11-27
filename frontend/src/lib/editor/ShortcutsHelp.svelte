<script lang="ts" module>
  import * as Blockly from 'blockly/core';

  const mac = /Mac|iPhone|iPad/.test(globalThis.navigator?.platform ?? '');
  const ctrl = mac ? '⌘' : 'Ctrl';

  const KEY_NAMES: Record<string, string> = {
    ENTER: 'Enter',
    SPACE: 'Space',
    ESC: 'Esc',
    DELETE: 'Delete',
    BACKSPACE: 'Backspace',
    LEFT: '←',
    RIGHT: '→',
    UP: '↑',
    DOWN: '↓',
    HOME: 'Home',
    END: 'End',
    PAGE_UP: 'Page Up',
    PAGE_DOWN: 'Page Down',
    CONTEXT_MENU: 'Menu',
    SLASH: '/',
    FORWARD_SLASH: '/',
  };
  const MODIFIERS: Record<string, string> = { Control: 'Ctrl', Meta: '⌘', Alt: mac ? '⌥' : 'Alt', Shift: 'Shift' };

  function keyName(code: number): string {
    const name = Object.entries(Blockly.utils.KeyCodes).find(([, v]) => v === code)?.[0];
    if (!name) return String.fromCharCode(code);
    return KEY_NAMES[name] ?? (name.length === 1 ? name : name.charAt(0) + name.slice(1).toLowerCase());
  }

  /** The keys of a Blockly shortcut as shown on this computer ("Ctrl+C", not also "⌘C"). */
  export function blocklyKeys(name: string): string[] {
    const out: string[] = [];
    for (const serialized of Blockly.ShortcutRegistry.registry.getKeyCodesByShortcutName(name)) {
      const parts = serialized.split('+');
      const code = Number(parts.pop());
      if (parts.includes(mac ? 'Control' : 'Meta')) continue;
      const text = [...parts.map((p) => MODIFIERS[p] ?? p), keyName(code)].join('+');
      if (!out.includes(text)) out.push(text);
    }
    return out;
  }
</script>

<script lang="ts">
  import { modal } from '../dialog';

  let { onclose }: { onclose: () => void } = $props();

  const N = Blockly.ShortcutItems.names;
  /** `instead`: shown in place of keys, for things without a shortcut. */
  type Row = [keys: string[], what: string, instead?: string];
  const fromBlockly = (name: string, what: string): Row => [blocklyKeys(name), what];

  const sections: [string, Row[]][] = [
    [
      'Puzzlebox',
      [
        [[`${ctrl}+Space`], 'Add a block by typing its name (also middle-click on empty space)'],
        [[`${ctrl}+S`], 'Save'],
        [[], 'Run the program; right-click a stack › Run this stack runs just that stack', 'Run button'],
        [['?'], 'Show these shortcuts'],
      ],
    ],
    [
      'Editing',
      [
        fromBlockly(N.COPY, 'Copy the selected block (paste it in any program, even in another tab)'),
        fromBlockly(N.CUT, 'Cut'),
        fromBlockly(N.PASTE, 'Paste'),
        fromBlockly(N.UNDO, 'Undo'),
        fromBlockly(N.REDO, 'Redo'),
        fromBlockly(N.DELETE, 'Delete the selected block'),
        fromBlockly(N.DUPLICATE, 'Duplicate the selected block'),
        fromBlockly(N.CLEANUP, 'Tidy up the blocks'),
      ],
    ],
    [
      'Moving around with the keyboard',
      [
        fromBlockly(N.FOCUS_WORKSPACE, 'Go to the blocks'),
        fromBlockly(N.FOCUS_TOOLBOX, 'Go to the block categories'),
        [['↑', '↓', '←', '→'], 'Move the cursor between blocks and the slots in them'],
        fromBlockly(N.PERFORM_ACTION, 'Edit the field, or put a block from the toolbox where the cursor is'),
        fromBlockly(N.MENU, 'Open the right-click menu'),
        fromBlockly(N.NEXT_STACK, 'Next stack of blocks'),
        fromBlockly(N.PREVIOUS_STACK, 'Previous stack of blocks'),
        fromBlockly(N.START_MOVE, 'Move the block (arrows, then Enter to drop it)'),
        fromBlockly(N.DISCONNECT, 'Take the block out of its stack'),
        fromBlockly(N.SHOW_TOOLTIP, 'Show the help text of the block'),
        fromBlockly(N.INFORMATION, 'Read out where the cursor is (screen readers)'),
        fromBlockly(N.ESCAPE, 'Close menus and pickers'),
      ],
    ],
  ];
</script>

<div class="backdrop" role="presentation" onclick={onclose}></div>
<div class="help" role="dialog" aria-labelledby="pb-shortcuts-title" use:modal={onclose}>
  <h3 id="pb-shortcuts-title">Keyboard shortcuts</h3>
  {#each sections as [title, rows] (title)}
    <h4>{title}</h4>
    <dl>
      {#each rows.filter(([keys, , instead]) => keys.length || instead) as [keys, what, instead] (what)}
        <dt>
          {#each keys as k, i (k)}{#if i}<span class="or">or</span>{/if}<kbd>{k}</kbd>{/each}
          {#if instead}<span class="instead">{instead}</span>{/if}
        </dt>
        <dd>{what}</dd>
      {/each}
    </dl>
  {/each}
  <div class="actions"><button class="btn" onclick={onclose}>Close</button></div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 30%);
    z-index: 80;
  }
  .help {
    position: fixed;
    z-index: 81;
    left: 50%;
    top: 8%;
    transform: translateX(-50%);
    width: min(560px, calc(100% - 32px));
    max-height: 84vh;
    overflow: auto;
    box-sizing: border-box;
    padding: 16px 20px;
    background: var(--pb-card);
    border-radius: 12px;
    box-shadow: 0 8px 28px rgb(0 0 0 / 25%);
  }
  h3 {
    margin: 0 0 4px;
    font-weight: 500;
  }
  h4 {
    margin: 16px 0 6px;
    font-size: 13px;
    font-weight: 500;
    color: var(--pb-text-2);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  dl {
    display: grid;
    grid-template-columns: 190px 1fr;
    gap: 6px 16px;
    margin: 0;
    font-size: 14px;
  }
  dt {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px;
  }
  dd {
    margin: 0;
  }
  kbd {
    display: inline-block;
    padding: 1px 6px;
    border: 1px solid var(--pb-divider);
    border-bottom-width: 2px;
    border-radius: 4px;
    font: inherit;
    font-size: 12px;
    white-space: nowrap;
    background: color-mix(in srgb, var(--pb-text) 4%, transparent);
  }
  .instead {
    font-size: 13px;
  }
  .or {
    font-size: 12px;
    color: var(--pb-text-2);
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    margin-top: 16px;
  }
  @media (max-width: 500px) {
    dl {
      grid-template-columns: 1fr;
      gap: 2px;
    }
    dd {
      margin-bottom: 8px;
    }
  }
</style>
