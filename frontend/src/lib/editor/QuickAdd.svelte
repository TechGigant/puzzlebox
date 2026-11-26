<script lang="ts">
  import { onMount } from 'svelte';
  import { search, type Entry } from './quickadd';

  let {
    entries,
    x,
    y,
    onpick,
    onclose,
  }: { entries: Entry[]; x: number; y: number; onpick: (e: Entry) => void; onclose: () => void } = $props();

  let query = $state('');
  let active = $state(0);
  let input: HTMLInputElement;
  let list: HTMLUListElement | undefined = $state();
  const results = $derived(search(entries, query));

  $effect(() => {
    void query;
    active = 0;
  });

  function key(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      onclose();
    } else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      if (!results.length) return;
      active = (active + (e.key === 'ArrowDown' ? 1 : -1) + results.length) % results.length;
      list?.children[active]?.scrollIntoView({ block: 'nearest' });
    } else if (e.key === 'Enter' && results[active]) {
      e.preventDefault();
      onpick(results[active]);
    }
  }

  onMount(() => input.focus());
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="backdrop" onmousedown={onclose}></div>
<div class="quick" style:left="{x}px" style:top="{y}px" role="dialog" aria-label="Add a block">
  <input
    bind:this={input}
    bind:value={query}
    onkeydown={key}
    placeholder="Type to add a block, e.g. turn on light"
    spellcheck="false"
    autocomplete="off"
  />
  {#if query.trim()}
    <ul bind:this={list} role="listbox">
      {#each results as r, i (i)}
        <li role="option" aria-selected={i === active} class:active={i === active}>
          <button onmousedown={(e) => (e.preventDefault(), onpick(r))} onmouseenter={() => (active = i)}>
            <span class="label">{r.label}</span>
            <span class="path">{r.path}</span>
          </button>
        </li>
      {:else}
        <li class="empty">No matching blocks</li>
      {/each}
    </ul>
  {:else}
    <p class="hint">Enter adds the block after the selected one, or where you clicked.</p>
  {/if}
</div>

<style>
  .backdrop {
    position: absolute;
    inset: 0;
    z-index: 70;
  }
  .quick {
    position: absolute;
    z-index: 71;
    width: min(460px, calc(100% - 32px));
    background: var(--pb-card);
    border: 1px solid var(--pb-divider);
    border-radius: 12px;
    box-shadow: 0 8px 28px rgb(0 0 0 / 25%);
    overflow: hidden;
  }
  input {
    width: 100%;
    box-sizing: border-box;
    border: 0;
    border-bottom: 1px solid var(--pb-divider);
    padding: 12px 14px;
    font: inherit;
    font-size: 15px;
    color: var(--pb-text);
    background: transparent;
    outline: none;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 4px;
    max-height: 340px;
    overflow-y: auto;
  }
  li button {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
    width: 100%;
    text-align: left;
    border: 0;
    background: none;
    color: inherit;
    font: inherit;
    padding: 6px 10px;
    border-radius: 8px;
    cursor: pointer;
  }
  li.active button {
    background: color-mix(in srgb, var(--pb-primary) 14%, transparent);
  }
  .label {
    font-size: 14px;
  }
  .path,
  .hint,
  .empty {
    font-size: 12px;
    color: var(--pb-text-2);
  }
  .hint,
  .empty {
    margin: 0;
    padding: 10px 14px;
  }
</style>
