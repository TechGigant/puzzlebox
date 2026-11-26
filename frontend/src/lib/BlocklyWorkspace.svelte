<script lang="ts" module>
  import * as Blockly from 'blockly/core';
  import 'blockly/blocks';
  import '@blockly/block-plus-minus';
  import * as En from 'blockly/msg/en';
  import { registerCoreBlocks } from './blocks/core';
  import { registerDeviceBlocks, registerDeviceCallbacks } from './blocks/devices';
  import { registry } from './registry.svelte';
  import { registerSwitchMenu } from './editor/switching';
  import { registerUsageMenu } from './editor/usages';

  Blockly.setLocale(En as unknown as Record<string, string>);
  registerCoreBlocks();
  registerDeviceBlocks();
  registerSwitchMenu();
  registerUsageMenu();

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
  import { insert, toolboxEntries, workspaceEntries, type Entry } from './editor/quickadd';
  import { applyZebra, loadPrefs, rendererName } from './editor/appearance';

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
  let quick = $state<{ entries: Entry[]; x: number; y: number; at: Blockly.utils.Coordinate } | null>(null);
  let mouse = { x: 80, y: 80, clientX: 0, clientY: 0 };
  let toolboxDef: Blockly.utils.toolbox.ToolboxInfo | null = null;

  function openQuickAdd() {
    if (!workspace || !toolboxDef) return;
    const rect = host.getBoundingClientRect();
    const x = Math.min(Math.max(mouse.x, 16), rect.width - 300);
    const y = Math.min(Math.max(mouse.y, 16), rect.height - 200);
    // Where the block lands if nothing is selected: under the mouse.
    const at = mouse.clientX
      ? Blockly.utils.svgMath.screenToWsCoordinates(workspace, new Blockly.utils.Coordinate(mouse.clientX, mouse.clientY))
      : new Blockly.utils.Coordinate(40, 40);
    quick = { entries: [...workspaceEntries(workspace), ...toolboxEntries(toolboxDef)], x, y, at };
  }

  function pick(e: Entry) {
    if (workspace && quick) insert(workspace, e.state, quick.at);
    quick = null;
    host.querySelector<HTMLElement>('.blocklySvg')?.focus();
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
      Blockly.serialization.workspaces.load(state, workspace);
    } else {
      const when = workspace.newBlock('ha_when');
      when.initSvg();
      when.render();
      when.moveBy(40, 40);
    }
    breakpoints.clear();
    // Blockly fires change events on a timer, so keep ignoring them until that has run.
    setTimeout(() => (loading = false), 0);
    workspace.scrollCenter();
  }

  export function save(): object {
    return workspace ? Blockly.serialization.workspaces.save(workspace) : {};
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
      byBlock.set(e.block_id, [...(byBlock.get(e.block_id) ?? []), e.message]);
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
    const b = workspace?.getBlockById(id);
    if (!b) return null;
    const text = b.toString(48).replace(/\s+/g, ' ').trim();
    return text || b.type;
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
    // The minimap copies the theme once at creation, so rebuild it when the theme changes.
    let minimap = new PositionedMinimap(workspace);
    minimap.init();
    const rebuildMinimap = () => {
      if (!workspace) return;
      minimap.dispose();
      // dispose() leaves the plugin registered as a positionable component.
      workspace.getComponentManager().removeComponent('minimap');
      minimap = new PositionedMinimap(workspace);
      minimap.init();
      // A new minimap only mirrors future events; seed it with the current blocks.
      const mini = (minimap as unknown as { minimapWorkspace: Blockly.WorkspaceSvg | null }).minimapWorkspace;
      if (mini) {
        Blockly.serialization.workspaces.load(Blockly.serialization.workspaces.save(workspace), mini);
        mini.zoomToFit();
      }
    };
    new WorkspaceSearch(workspace).init();

    const backpack = new Backpack(workspace, {
      skipSerializerRegistration: true,
      allowEmptyBackpackOpen: false,
      useFilledBackpackImage: true,
      contextMenu: { copyAllToBackpack: true, pasteAllToBackpack: true, emptyBackpack: true, removeFromBackpack: true },
    });
    backpack.init();
    // The backpack is shared by all programs and kept by the app, not in this program.
    let restoring = true;
    api
      .backpack()
      .then((c) => backpack.setContents(c))
      .catch(() => {})
      .finally(() => (restoring = false));

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
    host.addEventListener('keydown', onKey);
    host.addEventListener('mousedown', onMiddle);

    let zebraQueued = false;
    const zebra = () => {
      if (zebraQueued) return;
      zebraQueued = true;
      requestAnimationFrame(() => {
        zebraQueued = false;
        if (workspace) applyZebra(workspace, prefs.zebra);
      });
    };

    breakpointHandlers.set(workspace.id, toggleBreakpoint);
    breakpointSets.set(workspace.id, breakpoints);

    const stopTheme = watchTheme((t) => {
      tokens = t;
      applyCssTokens(t);
      workspace?.setTheme(buildTheme(t, extraStyles));
      rebuildMinimap();
    });

    workspace.addChangeListener((e) => {
      if (e.type === 'backpack_change' && !restoring) {
        api.setBackpack(backpack.getContents()).catch(() => {});
      }
      if (!e.isUiEvent || e.type === Blockly.Events.THEME_CHANGE) zebra();
      if (loading || e.isUiEvent || !onchange || !workspace) return;
      if (e.type === Blockly.Events.BLOCK_DELETE) {
        for (const id of (e as Blockly.Events.BlockDelete).ids ?? []) breakpoints.delete(id);
      }
      onchange(Blockly.serialization.workspaces.save(workspace));
    });

    load(initial);

    const resize = new ResizeObserver(() => workspace && Blockly.svgResize(workspace));
    resize.observe(host);

    return () => {
      resize.disconnect();
      host.removeEventListener('mousemove', onMouse);
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
  <QuickAdd entries={quick.entries} x={quick.x} y={quick.y} onpick={pick} onclose={() => (quick = null)} />
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
  .workspace :global(.blocklyMinimap) {
    border-radius: 12px;
    overflow: hidden;
    box-shadow: 0 2px 8px rgb(0 0 0 / 0.2);
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
  .workspace :global(.pb-usage > .blocklyPath) {
    stroke: var(--pb-primary);
    stroke-width: 3px;
  }
  .workspace :global(.pb-failed > .blocklyPath) {
    stroke: #e53935;
    stroke-width: 4px;
  }
</style>
