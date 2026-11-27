<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from './api';
  import { display, guessKind, kindOf, parse, type ValueKind } from './stored_values';

  const KINDS: ValueKind[] = ['text', 'number', 'yes/no', 'list', 'dictionary'];

  interface Row {
    name: string;
    kind: ValueKind;
    text: string;
    saved: string;
    savedKind: ValueKind;
    note?: { ok: boolean; text: string };
  }
  interface Group {
    /** Program id; undefined for global variables. */
    id?: string;
    title: string;
    rows: Row[];
  }

  let groups = $state<Group[] | null>(null);
  let newVar = $state({ name: '', text: '' });
  let newNote = $state<string | null>(null);

  const toRows = (vars: Record<string, unknown>): Row[] =>
    Object.entries(vars)
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([name, v]) => {
        const kind = kindOf(v) === 'empty' ? 'text' : kindOf(v);
        return { name, kind, text: display(v), saved: display(v), savedKind: kind };
      });

  async function refresh() {
    const [vars, programs] = await Promise.all([api.variables(), api.programs().catch(() => [])]);
    const names = new Map(programs.map((p) => [p.id, p.name]));
    const perProgram = Object.entries(vars.programs)
      .map(([id, v]) => ({ id, title: names.get(id) ?? 'A deleted program', rows: toRows(v) }))
      .sort((a, b) => a.title.localeCompare(b.title));
    groups = [{ title: 'Global (shared by all programs)', rows: toRows(vars.global) }, ...perProgram];
  }

  async function save(g: Group, r: Row) {
    let value: unknown;
    try {
      value = parse(r.text, r.kind);
    } catch (e) {
      r.note = { ok: false, text: (e as Error).message };
      return;
    }
    try {
      await api.setVariable(r.name, value, g.id);
      r.saved = r.text;
      r.savedKind = r.kind;
      r.note = { ok: true, text: 'Saved' };
      setTimeout(() => r.note?.ok && (r.note = undefined), 2500);
    } catch (e) {
      r.note = { ok: false, text: `Not saved: ${(e as Error).message}` };
    }
  }

  async function remove(g: Group, r: Row) {
    const where = g.id ? ` of “${g.title}”` : '';
    if (!confirm(`Delete the stored variable “${r.name}”${where}? Programs read it as empty until they set it again.`)) return;
    await api.deleteVariable(r.name, g.id);
    g.rows = g.rows.filter((x) => x !== r);
  }

  async function add() {
    const name = newVar.name.trim();
    const kind = guessKind(newVar.text);
    try {
      await api.setVariable(name, parse(newVar.text, kind), undefined);
    } catch (e) {
      newNote = (e as Error).message;
      return;
    }
    newVar = { name: '', text: '' };
    newNote = null;
    await refresh();
  }

  const dirty = (r: Row) => r.text !== r.saved || r.kind !== r.savedKind;

  onMount(refresh);
</script>

{#if groups}
  {#each groups as g (g.id ?? '')}
    <h3>{#if g.id}Program <a href={`#/p/${g.id}`}>{g.title}</a>{:else}{g.title}{/if}</h3>
    <table>
      <tbody>
        {#each g.rows as r (r.name)}
          <tr>
            <td>{r.name}</td>
            <td>
              <div class="row">
                <input
                  bind:value={r.text}
                  aria-label={`Value of ${r.name}`}
                  onkeydown={(e) => e.key === 'Enter' && dirty(r) && save(g, r)}
                />
                <select bind:value={r.kind} aria-label={`Kind of value of ${r.name}`}>
                  {#each KINDS as k (k)}<option value={k}>{k}</option>{/each}
                </select>
                <button class="btn" disabled={!dirty(r)} onclick={() => save(g, r)}>Save</button>
                <button class="btn" onclick={() => remove(g, r)} aria-label={`Delete ${r.name}`}>Delete</button>
              </div>
              {#if r.note}<span class="note" class:bad={!r.note.ok} class:ok={r.note.ok}>{r.note.text}</span>{/if}
            </td>
          </tr>
        {:else}
          {#if !g.id}<tr><td colspan="2" class="muted">No global variables yet.</td></tr>{/if}
        {/each}
        {#if !g.id}
          <tr>
            <td><input placeholder="name" bind:value={newVar.name} aria-label="New variable name" /></td>
            <td>
              <div class="row">
                <input placeholder="value" bind:value={newVar.text} aria-label="New variable value" />
                <span class="muted">{guessKind(newVar.text)}</span>
                <button class="btn" disabled={!newVar.name.trim()} onclick={add}>Add</button>
              </div>
              {#if newNote}<span class="note bad">{newNote}</span>{/if}
            </td>
          </tr>
        {/if}
      </tbody>
    </table>
  {/each}
{/if}

<style>
  h3 {
    font-size: 14px;
    font-weight: 500;
    color: var(--pb-text-2);
    margin: 16px 0 6px;
  }
  table {
    width: 100%;
    border-collapse: collapse;
  }
  td {
    padding: 4px;
    vertical-align: top;
  }
  td:first-child {
    width: 30%;
    font-weight: 500;
    padding-top: 9px;
  }
  td:first-child input {
    width: 100%;
    box-sizing: border-box;
  }
  .row {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .row input {
    flex: 1;
    min-width: 0;
  }
  .note {
    font-size: 12px;
  }
  .muted {
    color: var(--pb-text-2);
    font-size: 13px;
  }
  .ok {
    color: #43a047;
  }
  .bad {
    color: #e53935;
  }
  a {
    color: var(--pb-primary);
  }
</style>
