<script lang="ts" module>
  import * as Blockly from 'blockly/core';
  import 'blockly/blocks';
  import '@blockly/block-plus-minus';
  import * as En from 'blockly/msg/en';
  import { registerCoreBlocks } from './blocks/core';
  import { registerDeviceBlocks, registerDeviceCallbacks } from './blocks/devices';
  import { registerHelperBlocks } from './blocks/helpers';
  import { registerConditionBlocks } from './blocks/conditions';
  import { registry } from './registry.svelte';
  import { registerSwitchMenu } from './editor/switching';
  import { registerUsageMenu } from './editor/usages';
  import { noteUsed, registerFavourites, registerFavouritesMenu } from './editor/favourites';
  import { installDropdownSearch } from './editor/dropdown_search';

  Blockly.setLocale(En as unknown as Record<string, string>);
  registerCoreBlocks();
  registerDeviceBlocks();
  registerHelperBlocks();
  registerConditionBlocks();
  registerSwitchMenu();
  registerUsageMenu();
  registerFavouritesMenu();
  installDropdownSearch();

  /** Per-workspace callbacks for the editor that owns it (menus are registered globally). */
  const runStackHandlers = new Map<string, (topBlockId: string) => void>();
  const quickAddHandlers = new Map<string, () => void>();
  const ENTRY_BLOCKS = new Set(['ha_when', 'ha_on_run', 'ha_on_start']);

  Blockly.ContextMenuRegistry.registry.register({
    id: 'puzzlebox_run_stack',
    scopeType: Blockly.ContextMenuRegistry.ScopeType.BLOCK,
    weight: -1,
    displayText: (scope) => (ENTRY_BLOCKS.has(scope.block!.getRootBlock().type) ? 'Run these blocks now' : 'Run this stack'),
    preconditionFn: (scope) => {
      const b = scope.block;
      if (!b || b.isInFlyout || !runStackHandlers.has(b.workspace.id)) return 'hidden';
      const root = b.getRootBlock();
      // Entry blocks, or loose statement stacks (not values or triggers).
      if (ENTRY_BLOCKS.has(root.type)) return 'enabled';
      if (root.outputConnection || root.previousConnection?.getCheck()?.includes('Trigger')) return 'hidden';
      return root.previousConnection || root.nextConnection ? 'enabled' : 'hidden';
    },
    callback: (scope) => runStackHandlers.get(scope.block!.workspace.id)?.(scope.block!.getRootBlock().id),
  });

  Blockly.ContextMenuRegistry.registry.register({
    id: 'puzzlebox_quick_add',
    scopeType: Blockly.ContextMenuRegistry.ScopeType.WORKSPACE,
    weight: -10,
    displayText: () => 'Add a block… (Ctrl+Space)',
    preconditionFn: (scope) => (scope.workspace && quickAddHandlers.has(scope.workspace.id) ? 'enabled' : 'hidden'),
    callback: (scope) => quickAddHandlers.get(scope.workspace!.id)?.(),
  });

  const breakpointHandlers = new Map<string, (blockId: string) => void>();
  const breakpointSets = new Map<string, Set<string>>();

  // The plus/minus "if" block can add "else if" rows but has no control for "else".
  type IfBlock = Blockly.BlockSvg & { hasElse_?: boolean };
  Blockly.ContextMenuRegistry.registry.register({
    id: 'puzzlebox_toggle_else',
    scopeType: Blockly.ContextMenuRegistry.ScopeType.BLOCK,
    weight: 1,
    displayText: (scope) => ((scope.block as IfBlock).getInput('ELSE') ? 'Remove else' : 'Add else'),
    preconditionFn: (scope) =>
      scope.block?.type === 'controls_if' && !scope.block.isInFlyout && scope.block.isEditable() ? 'enabled' : 'hidden',
    callback: (scope) => {
      const b = scope.block as IfBlock;
      const before = JSON.stringify(b.saveExtraState?.() ?? null);
      if (b.getInput('ELSE')) {
        b.removeInput('ELSE');
        b.hasElse_ = false;
      } else {
        b.hasElse_ = true;
        b.appendStatementInput('ELSE').appendField(Blockly.Msg['CONTROLS_IF_MSG_ELSE']);
      }
      const after = JSON.stringify(b.saveExtraState?.() ?? null);
      const Change = Blockly.Events.get(Blockly.Events.BLOCK_CHANGE);
      Blockly.Events.fire(new Change(b, 'mutation', null, before, after));
    },
  });

  Blockly.ContextMenuRegistry.registry.register({
    id: 'puzzlebox_toggle_breakpoint',
    scopeType: Blockly.ContextMenuRegistry.ScopeType.BLOCK,
    weight: 0,
    displayText: (scope) => {
      const b = scope.block!;
      return breakpointSets.get(b.workspace.id)?.has(b.id) ? 'Remove breakpoint' : 'Add breakpoint';
    },
    preconditionFn: (scope) => {
      const b = scope.block!;
      if (b.isInFlyout || !breakpointHandlers.has(b.workspace.id)) return 'hidden';
      return b.previousConnection && b.type !== 'dict_pair' && b.type !== 'control_branch' && b.previousConnection.getCheck()?.[0] !== 'Trigger' ? 'enabled' : 'hidden';
    },
    callback: (scope) => breakpointHandlers.get(scope.block!.workspace.id)?.(scope.block!.id),
  });
</script>

<script lang="ts">
  import { onMount } from 'svelte';
  import { PositionedMinimap } from '@blockly/workspace-minimap';
  import { WorkspaceSearch } from '@blockly/plugin-workspace-search';
  import { buildTheme, applyCssTokens, readHaTokens, watchTheme, type HaTokens } from './theme';
  import { buildToolbox, registerStoredVariablesCategory, type ExtraCategory } from './toolbox';
  import { Backpack } from '@blockly/workspace-backpack';
  import { api, type CompileError } from './api';
  import QuickAdd from './editor/QuickAdd.svelte';
  import { blockWords, insert, isTrigger, selectedBlock, toolboxEntries, wantFor, workspaceEntries, type Entry, type Want } from './editor/quickadd';
  import { applyZebra, loadPrefs, markLoose, rendererName } from './editor/appearance';
  import { installDragTricks } from './editor/drag';
  import { installSummaries } from './editor/summaries';
  import { migrateWorkspace } from './editor/migrate';
  import { FieldEntity, knownEntities, loadEntities, loadRegistry as loadHaNames, refreshNameFields } from './blocks/field_entity';

  /** The plugin's minimap is a fifth of the editor wide and covers the blocks under it. */
  class SmallMinimap extends PositionedMinimap {
    override setSize() {
      this.width = 160;
      this.height = 100;
    }
  }

  let {
    initial,
    extraCategories = [],
    extraStyles = [],
    onchange,
    onbreakpoints,
    onrunstack,
  }: {
    initial?: object;
    extraCategories?: ExtraCategory[];
    extraStyles?: string[];
    onchange?: (state: object) => void;
    onbreakpoints?: (ids: string[]) => void;
    /** "Run this stack": the id of the stack's top block. */
    onrunstack?: (topBlockId: string) => void;
  } = $props();

  const prefs = loadPrefs();
  let quick = $state<{
    entries: Entry[];
    x: number;
    y: number;
    at: Blockly.utils.Coordinate;
    target: Blockly.BlockSvg | null;
    want: Want | null;
    hint: string;
  } | null>(null);
  let mouse = { x: 80, y: 80, clientX: 0, clientY: 0 };
  let toolboxDef: Blockly.utils.toolbox.ToolboxInfo | null = null;

  function quickHint(target: Blockly.BlockSvg | null, want: Want | null): string {
    if (!target) return 'Enter adds the block where you clicked (a trigger goes into an empty “when”).';
    if (want?.shape === 'trigger' || target.type === 'ha_when')
      return 'Triggers go into the selected “when”, actions at the end of its “do”.';
    if (want?.shape === 'value') return 'Values go into the first empty slot of the selected block.';
    return 'Enter adds the block after the selected one.';
  }

  function openQuickAdd() {
    if (!workspace || !toolboxDef) return;
    const rect = host.getBoundingClientRect();
    const x = Math.min(Math.max(mouse.x, 16), Math.max(16, rect.width - 480));
    // The popup is up to ~400px tall (search box and results); keep all of it on screen.
    const y = Math.min(Math.max(mouse.y, 16), Math.max(16, rect.height - 420));
    // Where the block lands if nothing is selected: under the mouse.
    const at = mouse.clientX
      ? Blockly.utils.svgMath.screenToWsCoordinates(workspace, new Blockly.utils.Coordinate(mouse.clientX, mouse.clientY))
      : new Blockly.utils.Coordinate(40, 40);
    // Read now: once the search box has focus, Blockly no longer reports a selection.
    const target = selectedBlock(workspace);
    const want = wantFor(target);
    quick = {
      entries: [...workspaceEntries(workspace), ...toolboxEntries(toolboxDef)],
      x,
      y,
      at,
      target,
      want,
      hint: quickHint(target, want),
    };
  }

  /** Gives the keyboard back to the blocks, so Ctrl+Space and shortcuts keep working. */
  function closeQuick() {
    const target = quick?.target;
    quick = null;
    if (target && !target.isDeadOrDying()) Blockly.getFocusManager().focusNode(target);
    else if (workspace) Blockly.getFocusManager().focusTree(workspace);
  }

  function pick(e: Entry) {
    const q = quick;
    quick = null;
    if (workspace && q) noteUsed(insert(workspace, e.state, q.at, q.target));
  }

  let host: HTMLDivElement;
  let workspace: Blockly.WorkspaceSvg | null = null;
  let tokens: HaTokens = readHaTokens();
  let loading = false;
  const breakpoints = new Set<string>();
  const marked = new Map<string, string>();
  let errorIds = new Set<string>();

  function blockEl(id: string): SVGElement | null {
    const b = workspace?.getBlockById(id) as Blockly.BlockSvg | null;
    return b?.getSvgRoot() ?? null;
  }

  function mark(id: string, cls: string, on: boolean) {
    blockEl(id)?.classList.toggle(cls, on);
  }

  export function load(state: object | undefined) {
    if (!workspace) return;
    loading = true;
    workspace.clear();
    if (state && Object.keys(state).length) {
      // A plain copy: the state may be a Svelte proxy, and the caller keeps the original.
      Blockly.serialization.workspaces.load(migrateWorkspace(JSON.parse(JSON.stringify(state))), workspace);
    } else {
      const when = workspace.newBlock('ha_when');
      when.initSvg();
      when.render();
      when.moveBy(40, 40);
    }
    breakpoints.clear();
    // Blockly fires change events on a timer, so keep ignoring them until that has run.
    setTimeout(() => (loading = false), 0);
    showStart();
  }

  /** Centres a program that fits on screen; a bigger one opens at its first (top-left) stack. */
  function showStart() {
    if (!workspace) return;
    const m = workspace.getMetrics();
    const scale = workspace.scale;
    const bounds = workspace.getBlocksBoundingBox();
    const first = workspace.getTopBlocks(true)[0];
    if (!first || ((bounds.right - bounds.left) * scale <= m.viewWidth && (bounds.bottom - bounds.top) * scale <= m.viewHeight)) {
      workspace.scrollCenter();
      return;
    }
    const xy = first.getRelativeToSurfaceXY();
    const margin = 24;
    workspace.scroll(-(xy.x * scale - margin), -(xy.y * scale - margin));
  }

  export function save(): object {
    return workspace ? Blockly.serialization.workspaces.save(workspace) : {};
  }

  /** Visible labels next to an input or field, e.g. "if" or "brightness". */
  function labelFor(b: Blockly.Block, name: string): string | null {
    for (const input of b.inputList) {
      const labels = input.fieldRow.filter((f) => f instanceof Blockly.FieldLabel).map((f) => f.getText().trim());
      if (input.name.toLowerCase() === name) return labels.join(' ') || null;
      const i = input.fieldRow.findIndex((f) => f.name?.toLowerCase() === name);
      if (i >= 0) {
        const before = input.fieldRow.slice(0, i).filter((f) => f instanceof Blockly.FieldLabel);
        return before.at(-1)?.getText().trim() || null;
      }
    }
    return null;
  }

  /** Error text with internal slot names replaced by what the block shows. */
  function friendly(e: CompileError): string {
    const b = e.block_id ? workspace?.getBlockById(e.block_id) : null;
    if (!b) return e.message;
    return e.message.replace(/\u201c([^\u201d]+)\u201d/g, (all, name: string) => {
      const label = labelFor(b, name.toLowerCase());
      return label ? `\u201c${label}\u201d` : name.match(/^[a-z]+\d*$/) ? 'empty' : all;
    }).replace('the empty empty slot', 'the empty slot').replace('fill in empty', 'fill in the empty box');
  }

  /** Problems for the side panel: friendly text, prefixed with the block they're on. */
  export function friendlyErrors(errors: CompileError[]): CompileError[] {
    return errors.map((e) => {
      const where = e.block_id ? describe(e.block_id) : null;
      const text = where ? `${where}: ${friendly(e)}` : friendly(e);
      return { ...e, message: text.charAt(0).toUpperCase() + text.slice(1) };
    });
  }

  /** Parts that compile but never run by themselves: empty entry blocks and loose blocks. */
  export function hints(): CompileError[] {
    if (!workspace) return [];
    const out: CompileError[] = [];
    const quote = (b: Blockly.Block) => {
      const t = blockWords(b, { header: true });
      return `“${t.length > 40 ? t.slice(0, 39) + '…' : t || b.type}”`;
    };
    for (const top of workspace.getTopBlocks(true)) {
      if (!top.isEnabled() || top.type.startsWith('procedures_def')) continue;
      if (ENTRY_BLOCKS.has(top.type)) {
        let b = top.getInputTargetBlock('DO');
        while (b && !b.isEnabled()) b = b.getNextBlock();
        if (!b) out.push({ block_id: top.id, message: `The ${quote(top)} block has nothing to do yet: put blocks in its “do”.` });
      } else if (top.outputConnection) {
        out.push({ block_id: top.id, message: `${quote(top)} isn't plugged into another block, so it isn't used.` });
      } else if (isTrigger(top)) {
        out.push({ block_id: top.id, message: `The trigger ${quote(top)} isn't in a “when”, so it never fires.` });
      } else if (top.previousConnection || top.nextConnection) {
        const n = top.getDescendants(false).filter((b) => !b.isShadow()).length;
        const what = n > 2 ? `${quote(top)} and the ${n - 1} blocks under it aren't` : n > 1 ? `${quote(top)} and the block under it aren't` : `${quote(top)} isn't`;
        const runs = n > 1 ? 'they only run' : 'it only runs';
        out.push({ block_id: top.id, message: `${what} in a “when”, so ${runs} when you right-click › Run this stack.` });
      }
    }
    return out;
  }

  export function setErrors(errors: CompileError[]) {
    if (!workspace) return;
    for (const id of errorIds) {
      const b = workspace.getBlockById(id);
      b?.setWarningText(null);
      mark(id, 'pb-error', false);
    }
    errorIds = new Set();
    const byBlock = new Map<string, string[]>();
    for (const e of errors) {
      if (!e.block_id) continue;
      byBlock.set(e.block_id, [...(byBlock.get(e.block_id) ?? []), friendly(e)]);
    }
    // Entities that Home Assistant doesn't know (typos, removed devices, examples).
    const known = knownEntities();
    if (known.length) {
      const ids = new Set(known.map((k) => k.entity_id));
      for (const b of workspace.getAllBlocks(false)) {
        for (const input of b.inputList) {
          for (const f of input.fieldRow) {
            if (!(f instanceof FieldEntity)) continue;
            for (const id of String(f.getValue() ?? '').split(',').map((x) => x.trim()).filter(Boolean)) {
              if (!ids.has(id) && /^[a-z_]+\.[a-z0-9_]+$/.test(id)) {
                const holder = b.isShadow() ? (b.getParent() ?? b) : b;
                byBlock.set(holder.id, [...(byBlock.get(holder.id) ?? []), `${id} doesn't exist in Home Assistant`]);
              }
            }
          }
        }
      }
    } else {
      loadEntities().then(() => knownEntities().length && setErrors(errors));
    }
    for (const [id, msgs] of byBlock) {
      const b = workspace.getBlockById(id);
      if (!b) continue;
      b.setWarningText(msgs.join('\n'));
      mark(id, 'pb-error', true);
      errorIds.add(id);
    }
  }

  /** Marks `id` with a state class; one block per state at a time. */
  export function setMarker(state: 'active' | 'paused' | 'failed', id: string | null) {
    const cls = `pb-${state}`;
    const prev = marked.get(cls);
    if (prev) mark(prev, cls, false);
    if (id) {
      mark(id, cls, true);
      marked.set(cls, id);
    } else {
      marked.delete(cls);
    }
  }

  export function clearMarkers() {
    for (const [cls, id] of marked) mark(id, cls, false);
    marked.clear();
  }

  /** Short human-readable label for a block (used in run traces). */
  export function describe(id: string): string | null {
    let b = workspace?.getBlockById(id);
    if (!b) return null;
    // A shadow is the default value in a socket ("choose…"); name the block it belongs to.
    while (b.isShadow() && b.getParent()) b = b.getParent()!;
    if (ENTRY_BLOCKS.has(b.type)) return `the “${blockWords(b, { header: true }) || 'when'}” block`;
    // C-shaped blocks: just the part above their first slot of blocks ("if … do").
    const text = blockWords(b, { header: true });
    return text.length > 48 ? text.slice(0, 47) + '…' : text || b.type;
  }

  export function reveal(id: string) {
    const b = workspace?.getBlockById(id) as Blockly.BlockSvg | null;
    if (b) workspace?.centerOnBlock(b.id);
  }

  function toggleBreakpoint(id: string) {
    if (breakpoints.has(id)) breakpoints.delete(id);
    else breakpoints.add(id);
    mark(id, 'pb-breakpoint', breakpoints.has(id));
    onbreakpoints?.([...breakpoints]);
  }

  $effect(() => {
    if (!workspace) return;
    registerDeviceCallbacks(workspace, registry.ha);
    toolboxDef = buildToolbox(extraCategories);
    workspace.updateToolbox(toolboxDef);
    workspace.setTheme(buildTheme(tokens, extraStyles));
  });

  onMount(() => {
    applyCssTokens(tokens);
    toolboxDef = buildToolbox(extraCategories);
    workspace = Blockly.inject(host, {
      renderer: rendererName(prefs),
      media: 'blockly-media/',
      theme: buildTheme(tokens, extraStyles),
      toolbox: toolboxDef,
      grid: { spacing: 24, length: 2, colour: tokens.divider, snap: true },
      zoom: { controls: true, wheel: true, startScale: 0.75, maxScale: 2, minScale: 0.3, scaleSpeed: 1.15 },
      move: { scrollbars: true, drag: true, wheel: false },
      trashcan: true,
      sounds: false,
      maxTrashcanContents: 64,
    });
    registerStoredVariablesCategory(workspace);
    registerDeviceCallbacks(workspace, registry.ha);
    // The minimap copies the theme once at creation, so it is rebuilt when the theme changes.
    // Narrow screens get none: there it would cover the blocks (and the plugin fails to start).
    let minimap: SmallMinimap | null = null;
    const syncMinimap = (rebuild = false) => {
      if (!workspace) return;
      const want = host.clientWidth >= 700;
      if (minimap && (rebuild || !want)) {
        // The plugin's window resize listener outlives dispose() and then throws on every resize.
        const region = (minimap as unknown as { focusRegion: { update: () => void } | null }).focusRegion;
        if (region) region.update = () => {};
        minimap.dispose();
        // dispose() leaves the plugin registered as a positionable component.
        workspace.getComponentManager().removeComponent('minimap');
        minimap = null;
      }
      if (!want || minimap) return;
      minimap = new SmallMinimap(workspace);
      minimap.init();
      // A new minimap only mirrors future events; seed it with the current blocks.
      const mini = (minimap as unknown as { minimapWorkspace: Blockly.WorkspaceSvg | null }).minimapWorkspace;
      if (mini && workspace.getTopBlocks(false).length) {
        Blockly.serialization.workspaces.load(Blockly.serialization.workspaces.save(workspace), mini);
        mini.zoomToFit();
      }
    };
    syncMinimap();
    new WorkspaceSearch(workspace).init();

    const backpack = new Backpack(workspace, {
      skipSerializerRegistration: true,
      allowEmptyBackpackOpen: false,
      useFilledBackpackImage: true,
      contextMenu: { copyAllToBackpack: true, pasteAllToBackpack: true, emptyBackpack: true, removeFromBackpack: true },
    });
    backpack.init();
    // The backpack is shared by all programs and kept by the app, not in this program.
    // Until the stored backpack has been read, changes aren't saved (they would overwrite it).
    let restoring = true;
    api
      .editorData<string>('backpack')
      .then((c) => {
        backpack.setContents([...c, ...backpack.getContents().filter((x) => !c.includes(x))]);
        restoring = false;
      })
      .catch(() => {});

    const wsId = workspace.id;
    runStackHandlers.set(wsId, (id) => onrunstack?.(id));
    quickAddHandlers.set(wsId, openQuickAdd);
    const onMouse = (e: MouseEvent) => {
      const r = host.getBoundingClientRect();
      mouse = { x: e.clientX - r.left, y: e.clientY - r.top, clientX: e.clientX, clientY: e.clientY };
    };
    const onKey = (e: KeyboardEvent) => {
      const typing = (e.target as HTMLElement)?.closest('input, textarea, [contenteditable]');
      if (e.code === 'Space' && (e.ctrlKey || e.metaKey) && !typing) {
        e.preventDefault();
        openQuickAdd();
      }
    };
    const onMiddle = (e: MouseEvent) => {
      if (e.button !== 1 || !(e.target as Element)?.classList?.contains('blocklyMainBackground')) return;
      e.preventDefault();
      onMouse(e);
      openQuickAdd();
    };
    host.addEventListener('mousemove', onMouse);
    registerFavourites(workspace);
    const stopDrag = installDragTricks(workspace, host, () => (mouse.clientX ? mouse : null));
    const stopSummaries = installSummaries(workspace);
    host.addEventListener('keydown', onKey);
    host.addEventListener('mousedown', onMiddle);

    let zebraQueued = false;
    const zebra = () => {
      if (zebraQueued) return;
      zebraQueued = true;
      requestAnimationFrame(() => {
        zebraQueued = false;
        if (workspace) {
          applyZebra(workspace, prefs.zebra);
          markLoose(workspace);
        }
      });
    };

    breakpointHandlers.set(workspace.id, toggleBreakpoint);
    breakpointSets.set(workspace.id, breakpoints);

    const stopTheme = watchTheme((t) => {
      tokens = t;
      applyCssTokens(t);
      workspace?.setTheme(buildTheme(t, extraStyles));
      syncMinimap(true);
    });

    workspace.addChangeListener((e) => {
      if (e.type === 'backpack_change' && !restoring) {
        api.setEditorData('backpack', backpack.getContents()).catch(() => {});
      }
      if (!e.isUiEvent || e.type === Blockly.Events.THEME_CHANGE) zebra();
      // A block created while a drag is under way came out of the toolbox.
      if (e.type === Blockly.Events.BLOCK_CREATE && !loading && workspace?.isDragging()) {
        const created = workspace.getBlockById((e as Blockly.Events.BlockCreate).blockId ?? '');
        if (created && !created.getParent()) noteUsed(created);
      }
      if (loading || e.isUiEvent || !onchange || !workspace) return;
      if (e.type === Blockly.Events.BLOCK_DELETE) {
        for (const id of (e as Blockly.Events.BlockDelete).ids ?? []) breakpoints.delete(id);
      }
      onchange(Blockly.serialization.workspaces.save(workspace));
    });

    load(initial);
    // Pickers show names, which need the entity and area lists.
    Promise.all([loadEntities(), loadHaNames()]).then(() => workspace && refreshNameFields(workspace));

    const resize = new ResizeObserver(() => {
      if (!workspace) return;
      syncMinimap();
      Blockly.svgResize(workspace);
    });
    resize.observe(host);

    return () => {
      resize.disconnect();
      host.removeEventListener('mousemove', onMouse);
      stopDrag();
      stopSummaries();
      host.removeEventListener('keydown', onKey);
      host.removeEventListener('mousedown', onMiddle);
      runStackHandlers.delete(wsId);
      quickAddHandlers.delete(wsId);
      stopTheme();
      if (workspace) {
        breakpointHandlers.delete(workspace.id);
        breakpointSets.delete(workspace.id);
        workspace.dispose();
      }
      workspace = null;
    };
  });
</script>

<div class="workspace" bind:this={host}></div>
{#if quick}
  <QuickAdd
    entries={quick.entries}
    x={quick.x}
    y={quick.y}
    want={quick.want}
    hint={quick.hint}
    onpick={pick}
    onclose={closeQuick}
  />
{/if}

<style>
  .workspace {
    position: absolute;
    inset: 0;
  }
  .workspace :global(.blocklyToolbox) {
    border-right: 1px solid var(--pb-divider);
    padding-top: 8px;
  }
  .workspace :global(.blocklyToolboxCategory) {
    border-radius: 0 20px 20px 0;
    margin: 2px 8px 2px 0;
    height: 36px;
    line-height: 36px;
  }
  .workspace :global(.blocklyToolboxCategoryLabel) {
    font-family: inherit;
    font-size: 14px;
    font-weight: 500;
  }
  .workspace :global(.blocklyTreeSeparator),
  .workspace :global(.blocklyToolboxSeparator) {
    border-bottom-color: var(--pb-divider);
  }
  .workspace :global(.blocklyMainBackground) {
    stroke: none;
  }
  .workspace :global(.blockly-minimap) {
    border-radius: 12px;
    overflow: hidden;
    box-shadow: 0 2px 8px rgb(0 0 0 / 0.2);
    opacity: 0.45;
    transition: opacity 0.15s;
  }
  .workspace :global(.blockly-minimap:hover) {
    opacity: 0.95;
  }
  /* The app's text box style (padding, tinted background) would let the field's own text show through. */
  :global(.blocklyWidgetDiv input.blocklyHtmlInput) {
    padding: 0;
    border: 0;
    background: #fff;
    color: #000;
  }
  .workspace :global(.pb-breakpoint > .blocklyPath) {
    stroke: #e53935;
    stroke-width: 3px;
    stroke-dasharray: 6 3;
  }
  .workspace :global(.pb-error > .blocklyPath) {
    stroke: #ffb300;
    stroke-width: 3px;
  }
  .workspace :global(.pb-active > .blocklyPath) {
    stroke: var(--pb-primary);
    stroke-width: 4px;
    filter: drop-shadow(0 0 6px var(--pb-primary));
  }
  .workspace :global(.pb-paused > .blocklyPath) {
    stroke: #e53935;
    stroke-width: 4px;
    filter: drop-shadow(0 0 8px #e53935);
  }
  .workspace :global(.pb-loose) {
    opacity: 0.6;
  }
  .workspace :global(.pb-loose:hover),
  .workspace :global(.pb-loose.blocklySelected) {
    opacity: 0.9;
  }
  :global(.pb-dropdown-search) {
    display: block;
    box-sizing: border-box;
    width: calc(100% - 12px);
    margin: 6px;
    padding: 6px 8px;
    border: 1px solid var(--pb-divider);
    border-radius: 6px;
    font: inherit;
    font-size: 13px;
    color: var(--pb-text);
    background: var(--pb-card);
  }
  .workspace :global(.pb-usage > .blocklyPath) {
    stroke: var(--pb-primary);
    stroke-width: 3px;
  }
  .workspace :global(.pb-failed > .blocklyPath) {
    stroke: #e53935;
    stroke-width: 4px;
  }
</style>
