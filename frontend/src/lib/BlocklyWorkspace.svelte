<script lang="ts" module>
  import * as Blockly from 'blockly/core';
  import 'blockly/blocks';
  import '@blockly/block-plus-minus';
  import * as En from 'blockly/msg/en';
  import { registerCoreBlocks } from './blocks/core';

  Blockly.setLocale(En as unknown as Record<string, string>);
  registerCoreBlocks();

  const breakpointHandlers = new Map<string, (blockId: string) => void>();
  const breakpointSets = new Map<string, Set<string>>();

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
  import type { CompileError } from './api';

  let {
    initial,
    extraCategories = [],
    extraStyles = [],
    onchange,
    onbreakpoints,
  }: {
    initial?: object;
    extraCategories?: ExtraCategory[];
    extraStyles?: string[];
    onchange?: (state: object) => void;
    onbreakpoints?: (ids: string[]) => void;
  } = $props();

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
    workspace.updateToolbox(buildToolbox(extraCategories));
    workspace.setTheme(buildTheme(tokens, extraStyles));
  });

  onMount(() => {
    applyCssTokens(tokens);
    workspace = Blockly.inject(host, {
      renderer: 'zelos',
      media: 'blockly-media/',
      theme: buildTheme(tokens, extraStyles),
      toolbox: buildToolbox(extraCategories),
      grid: { spacing: 24, length: 2, colour: tokens.divider, snap: true },
      zoom: { controls: true, wheel: true, startScale: 0.75, maxScale: 2, minScale: 0.3, scaleSpeed: 1.15 },
      move: { scrollbars: true, drag: true, wheel: false },
      trashcan: true,
      sounds: false,
      maxTrashcanContents: 64,
    });
    registerStoredVariablesCategory(workspace);
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

    breakpointHandlers.set(workspace.id, toggleBreakpoint);
    breakpointSets.set(workspace.id, breakpoints);

    const stopTheme = watchTheme((t) => {
      tokens = t;
      applyCssTokens(t);
      workspace?.setTheme(buildTheme(t, extraStyles));
      rebuildMinimap();
    });

    workspace.addChangeListener((e) => {
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
  .workspace :global(.pb-failed > .blocklyPath) {
    stroke: #e53935;
    stroke-width: 4px;
  }
</style>
