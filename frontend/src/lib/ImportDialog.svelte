<script lang="ts">
  import { onMount } from 'svelte';
  import { api, type ImportedProgram } from './api';
  import { modal } from './dialog';

  let { onclose, ondone }: { onclose: () => void; ondone: (count: number) => void } = $props();

  type Item = ImportedProgram & { error?: string; entity_id?: string; selected: boolean };

  let tab = $state<'ha' | 'yaml'>('ha');
  let automations = $state<{ entity_id: string; name: string; state: string }[]>([]);
  let picked = $state<Set<string>>(new Set());
  let yaml = $state('');
  let preview = $state<Item[] | null>(null);
  let busy = $state(false);
  let error = $state<string | null>(null);

  onMount(async () => {
    automations = await api.automations().catch(() => []);
  });

  function toggle(id: string) {
    const next = new Set(picked);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    picked = next;
  }

  async function runPreview() {
    busy = true;
    error = null;
    try {
      const items = tab === 'ha' ? await api.importAutomations([...picked]) : await api.importYaml(yaml);
      preview = items.map((i) => ({ ...i, selected: !('error' in i && i.error) }));
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  /** Programs imported from HA automations, which can take over from them. */
  let imported = $state<{ id: string; name: string; entity_id: string; switched: boolean }[] | null>(null);
  let count = $state(0);

  async function create() {
    if (!preview) return;
    busy = true;
    error = null;
    const fromHa: NonNullable<typeof imported> = [];
    count = 0;
    try {
      for (const item of preview.filter((p) => p.selected && !p.error)) {
        const p = await api.create({
          name: item.name,
          description: item.description,
          mode: item.mode,
          max_runs: item.max_runs,
          workspace: item.workspace,
          // Disabled so the program doesn't run alongside the original automation.
          enabled: false,
        });
        count++;
        if (item.entity_id) fromHa.push({ id: p.id, name: p.name, entity_id: item.entity_id, switched: false });
      }
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
    if (fromHa.length) imported = fromHa;
    else if (!error) ondone(count);
  }

  async function switchOver(item: NonNullable<typeof imported>[number]) {
    if (!confirm(`Turn off the Home Assistant automation ${item.entity_id} and turn on “${item.name}”?\n\nYou can switch back any time: turn the automation on in Home Assistant and the program off here.`)) return;
    busy = true;
    error = null;
    try {
      await api.switchOver(item.id, item.entity_id);
      item.switched = true;
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }
</script>

<!-- Clicking outside doesn't close it: that would lose pasted YAML. -->
<div class="backdrop" role="presentation">
  <div class="dialog" role="dialog" aria-labelledby="pb-import-title" use:modal={() => (imported ? ondone(count) : onclose())}>
    <h2 id="pb-import-title">Import automations</h2>
    {#if imported}
      <p>Imported {count === 1 ? '1 program' : `${count} programs`}, turned off. To let a program take over, switch over: this turns the automation off in Home Assistant and the program on.</p>
      <div class="list">
        {#each imported as item (item.id)}
          <div class="row">
            <span class="name">{item.name}</span>
            <span class="muted">{item.entity_id}</span>
            {#if item.switched}
              <span class="ok">switched over</span>
            {:else}
              <button class="btn" disabled={busy} onclick={() => switchOver(item)}>Switch over</button>
            {/if}
          </div>
        {/each}
      </div>
      {#if error}<p class="error">{error}</p>{/if}
      <div class="actions">
        <button class="btn primary" onclick={() => ondone(count)}>Done</button>
      </div>
    {:else if !preview}
      <nav class="tabs">
        <button class:active={tab === 'ha'} onclick={() => (tab = 'ha')}>From Home Assistant</button>
        <button class:active={tab === 'yaml'} onclick={() => (tab = 'yaml')}>Paste YAML</button>
      </nav>
      {#if tab === 'ha'}
        <div class="list">
          {#each automations as a (a.entity_id)}
            <label class="row">
              <input type="checkbox" checked={picked.has(a.entity_id)} onchange={() => toggle(a.entity_id)} />
              <span class="name">{a.name}</span>
              <span class="muted">{a.entity_id}</span>
            </label>
          {:else}
            <p class="muted">No automations found.</p>
          {/each}
        </div>
      {:else}
        <textarea bind:value={yaml} spellcheck="false" placeholder="Paste an automation or script (or a list of them)"></textarea>
      {/if}
      {#if error}<p class="error">{error}</p>{/if}
      <div class="actions">
        <button class="btn" onclick={onclose}>Cancel</button>
        <button class="btn primary" disabled={busy || (tab === 'ha' ? picked.size === 0 : !yaml.trim())} onclick={runPreview}>Preview</button>
      </div>
    {:else}
      <div class="list">
        {#each preview as item, i (i)}
          <label class="row">
            <input type="checkbox" bind:checked={item.selected} disabled={!!item.error} />
            <span class="name">{item.name}</span>
            {#if item.error}
              <span class="error">{item.error}</span>
            {:else if item.raw_blocks}
              <span class="muted" title="These parts are kept as Home Assistant YAML blocks: they work unchanged but can't be edited as blocks.">{item.raw_blocks} part(s) kept as YAML</span>
            {:else}
              <span class="ok">fully converted</span>
            {/if}
          </label>
          {#if item.warnings?.length}
            <ul class="warnings">
              {#each item.warnings as w, j (j)}<li>{w}</li>{/each}
            </ul>
          {/if}
        {/each}
      </div>
      {#if error}<p class="error">{error}</p>{/if}
      <p class="muted note">Imported programs start <b>disabled</b>. Turn off the original automation before enabling its program, or both will run{preview.some((p) => p.entity_id) ? ': the next step offers to do that for you' : ''}.</p>
      <div class="actions">
        <button class="btn" onclick={() => (preview = null)}>Back</button>
        <button class="btn primary" disabled={busy || !preview.some((p) => p.selected)} onclick={create}>
          Import {preview.filter((p) => p.selected).length}
        </button>
      </div>
    {/if}
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 0.4);
    display: grid;
    place-items: center;
    z-index: 100;
    padding: 16px;
  }
  .dialog {
    background: var(--pb-card);
    border-radius: 16px;
    padding: 20px 24px;
    width: min(640px, 100%);
    max-height: calc(100vh - 32px);
    display: flex;
    flex-direction: column;
    gap: 12px;
    box-shadow: 0 8px 32px rgb(0 0 0 / 0.3);
  }
  h2 {
    margin: 0;
    font-weight: 400;
  }
  .tabs {
    display: flex;
    gap: 8px;
  }
  .tabs button {
    background: none;
    border: 0;
    border-bottom: 2px solid transparent;
    padding: 8px 4px;
    color: var(--pb-text-2);
    font: inherit;
    font-weight: 500;
    cursor: pointer;
  }
  .tabs button.active {
    color: var(--pb-primary);
    border-bottom-color: var(--pb-primary);
  }
  .list {
    overflow: auto;
    max-height: 50vh;
    border: 1px solid var(--pb-divider);
    border-radius: 8px;
  }
  .row {
    display: flex;
    gap: 10px;
    align-items: center;
    padding: 8px 12px;
    border-bottom: 1px solid var(--pb-divider);
  }
  .row:last-child {
    border-bottom: 0;
  }
  .name {
    flex: 1;
  }
  textarea {
    min-height: 280px;
    font-family: ui-monospace, monospace;
    font-size: 13px;
    background: color-mix(in srgb, var(--pb-text) 5%, transparent);
    color: var(--pb-text);
    border: 1px solid var(--pb-divider);
    border-radius: 8px;
    padding: 10px;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  .muted {
    color: var(--pb-text-2);
    font-size: 13px;
  }
  .ok {
    color: #43a047;
    font-size: 13px;
  }
  .error {
    color: #e53935;
    font-size: 13px;
  }
  .note {
    margin: 0;
  }
  .warnings {
    margin: -2px 0 6px 30px;
    padding-left: 16px;
    color: #ef6c00;
    font-size: 13px;
  }
</style>
