<script lang="ts">
  // Blocks are registered by BlocklyWorkspace, which the editor has already loaded.
  import * as Blockly from 'blockly/core';
  import { onMount } from 'svelte';
  import { buildTheme, readHaTokens } from './theme';
  import { loadPrefs, rendererName } from './editor/appearance';
  import { migrateWorkspace } from './editor/migrate';

  let { workspace, extraStyles = [] }: { workspace: object; extraStyles?: string[] } = $props();
  let host: HTMLDivElement;
  let problem = $state<string | null>(null);

  onMount(() => {
    const tokens = readHaTokens();
    const ws = Blockly.inject(host, {
      readOnly: true,
      renderer: rendererName(loadPrefs()),
      media: 'blockly-media/',
      theme: buildTheme(tokens, extraStyles),
      zoom: { controls: true, wheel: true, startScale: 0.6, maxScale: 2, minScale: 0.2 },
      move: { scrollbars: true, drag: true, wheel: false },
      sounds: false,
    });
    try {
      Blockly.serialization.workspaces.load(migrateWorkspace(JSON.parse(JSON.stringify(workspace))), ws);
      ws.scrollCenter();
    } catch (e) {
      problem = `Some blocks of this version can't be shown: ${(e as Error).message}`;
    }
    return () => ws.dispose();
  });
</script>

<div class="preview" bind:this={host}></div>
{#if problem}<p class="problem">{problem}</p>{/if}

<style>
  .preview {
    width: 100%;
    box-sizing: border-box;
    height: min(55vh, 480px);
    border: 1px solid var(--pb-divider);
    border-radius: 8px;
    overflow: hidden;
  }
  .problem {
    color: #e53935;
    font-size: 13px;
  }
</style>
