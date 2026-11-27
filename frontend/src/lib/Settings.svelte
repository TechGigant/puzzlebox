<script lang="ts">
  import { onMount } from 'svelte';
  import { api, apiUrl, type PluginsInfo, type Settings } from './api';
  import { loadRegistry, registry } from './registry.svelte';
  import { loadPrefs, savePrefs } from './editor/appearance';

  let prefs = $state(loadPrefs());
  $effect(() => savePrefs({ ...prefs }));

  let settings = $state<Settings | null>(null);
  let plugins = $state<PluginsInfo | null>(null);
  let vars = $state<{ global: Record<string, unknown>; programs: Record<string, Record<string, unknown>> } | null>(null);
  let pluginErrors = $state<{ path: string; error: string }[]>([]);
  let showToken = $state(false);
  let message = $state<string | null>(null);
  let newVar = $state({ name: '', value: '' });

  async function refresh() {
    [settings, plugins, vars] = await Promise.all([api.settings(), api.plugins(), api.variables()]);
  }

  async function reload() {
    const r = await api.reloadPlugins();
    pluginErrors = r.errors;
    await loadRegistry();
    await refresh();
    message = 'Plugins and block packs reloaded';
  }

  function parseValue(text: string): unknown {
    try {
      return JSON.parse(text);
    } catch {
      return text;
    }
  }

  async function saveVar(name: string, text: string, programId?: string) {
    await api.setVariable(name, parseValue(text), programId);
    await refresh();
  }

  async function copy(text: string) {
    await navigator.clipboard?.writeText(text);
    message = 'Copied';
  }

  onMount(refresh);
</script>

<div class="page">
  {#if message}<p class="message">{message}</p>{/if}

  <section>
    <h2>Editor</h2>
    <label class="check"><input type="checkbox" bind:checked={prefs.zebra} /> Alternate shades for blocks nested in blocks of the same colour</label>
    <label class="check"><input type="checkbox" bind:checked={prefs.compact} /> Compact blocks (smaller, for big programs)</label>
    <p class="muted">
      Applies when you next open a program; saved in this browser. In the editor, press <kbd>Ctrl</kbd>+<kbd>Space</kbd>
      (or middle-click the canvas) to add a block by typing, and right-click a block to switch it to a related one, find
      its uses or run its stack.
    </p>
  </section>

  <section>
    <h2>Home Assistant integration</h2>
    {#if settings}
      <p>
        Bridge:
        <b class:ok={settings.bridge_connected} class:bad={!settings.bridge_connected}>
          {settings.bridge_connected ? 'connected' : 'not connected'}
        </b>
        {#if settings.bridge.connected}
          <span class="muted">· integration {settings.bridge.integration_version ?? 'from before 0.3.0'}, protocol {settings.bridge.protocol}</span>
        {/if}
      </p>
      {#if settings.bridge.connected && settings.bridge.missing?.length}
        <p class="muted">Not available until the integration is updated: {settings.bridge.missing.join(', ')}.</p>
      {/if}
      {#if !settings.bridge_connected}
        <div class="warn">
          <p>Programs run without it, but until the integration connects these don't work:</p>
          <ul>
            <li>a switch and a “run” button for each program in Home Assistant, and its status sensor</li>
            <li>the <code>puzzlebox.run_program</code> and <code>puzzlebox.set_variable</code> actions, and the <code>puzzlebox_program_started</code>/<code>_finished</code> events</li>
            <li>webhook triggers (their URLs are served by Home Assistant)</li>
            <li>blocks added by other integrations</li>
          </ul>
          <p>
            To fix it: install the <b>Puzzlebox</b> integration (HACS → search “Puzzlebox”), restart Home Assistant, then check
            Settings → Devices &amp; services. {settings.supervised
              ? 'It finds this app by itself; if it shows an error there, open it and reconfigure.'
              : "Add it there with this app's URL and the token below."}
          </p>
        </div>
      {/if}
      {#if settings.supervised}
        <p class="muted">Install the <b>Puzzlebox</b> integration (HACS) and restart Home Assistant; it finds this app and sets itself up.</p>
      {:else}
        <p class="muted">Install the <b>Puzzlebox</b> integration in Home Assistant and enter this app's URL and the token below.</p>
      {/if}
      <div class="row">
        <code class="token">{showToken ? settings.bridge_token : '•'.repeat(24)}</code>
        <button class="btn" onclick={() => (showToken = !showToken)}>{showToken ? 'Hide' : 'Show'}</button>
        <button class="btn" onclick={() => copy(settings!.bridge_token)}>Copy token</button>
      </div>
      <p>MQTT: <b class:ok={settings.mqtt_enabled}>{settings.mqtt_enabled ? 'connected to broker' : 'not configured'}</b></p>
      {#if settings.webhooks.length}
        <h3>Webhooks</h3>
        <ul>
          {#each settings.webhooks as w (w)}
            <li><code>/api/webhook/{w}</code> <span class="muted">(on your Home Assistant URL)</span>
              <button class="link" onclick={() => copy(new URL(`/api/webhook/${w}`, window.parent?.location?.origin ?? apiUrl('')).toString())}>copy</button></li>
          {/each}
        </ul>
      {/if}
    {/if}
  </section>

  <section>
    <div class="head">
      <h2>Plugins &amp; block packs</h2>
      <button class="btn" onclick={reload}>Reload</button>
    </div>
    {#if plugins}
      <h3>WASM plugins</h3>
      {#each plugins.wasm as p (p.id)}
        <div class="card">
          <b>{p.name}</b> <span class="muted">{p.id} {p.version} · {p.blocks} blocks</span>
          {#if p.description}<p>{p.description}</p>{/if}
          <p class="muted">
            Permissions:
            {#if !p.capabilities.http.length && !p.capabilities.services.length && !p.capabilities.states && !p.capabilities.kv}none{/if}
            {#if p.capabilities.states}read states;{/if}
            {#if p.capabilities.kv}storage;{/if}
            {#if p.capabilities.services.length}call {p.capabilities.services.join(', ')};{/if}
            {#if p.capabilities.http.length}internet: {p.capabilities.http.join(', ')}{/if}
          </p>
        </div>
      {:else}
        <p class="muted">None. Put plugin folders in {plugins.wasm_dirs.join(' or ')}.</p>
      {/each}
      <h3>From Home Assistant integrations</h3>
      {#if plugins.python.length}
        <ul>{#each plugins.python as p (p)}<li>{p}</li>{/each}</ul>
      {:else}
        <p class="muted">No integration has registered blocks.</p>
      {/if}
      <h3>Block packs</h3>
      {#each registry.packs as p (p.id)}
        <div class="card"><b>{p.name}</b> <span class="muted">{p.id} {p.version} · {p.blocks} blocks</span></div>
      {:else}
        <p class="muted">None. Put pack files (.yaml) in {plugins.packs_dirs.join(' or ')}.</p>
      {/each}
      {#each [...pluginErrors, ...registry.packErrors] as e (e.path)}
        <p class="bad">{e.path}: {e.error}</p>
      {/each}
    {/if}
  </section>

  <section>
    <h2>Stored variables</h2>
    {#if vars}
      <h3>Global</h3>
      <table>
        <tbody>
          {#each Object.entries(vars.global) as [k, v] (k)}
            <tr>
              <td>{k}</td>
              <td><input value={JSON.stringify(v)} onchange={(e) => saveVar(k, (e.target as HTMLInputElement).value)} /></td>
            </tr>
          {/each}
          <tr>
            <td><input placeholder="name" bind:value={newVar.name} /></td>
            <td class="row">
              <input placeholder="value (JSON or text)" bind:value={newVar.value} />
              <button class="btn" disabled={!newVar.name} onclick={() => saveVar(newVar.name, newVar.value).then(() => (newVar = { name: '', value: '' }))}>Add</button>
            </td>
          </tr>
        </tbody>
      </table>
      {#each Object.entries(vars.programs) as [pid, pv] (pid)}
        <h3>Program <a href={`#/p/${pid}`}>{pid}</a></h3>
        <table>
          <tbody>
            {#each Object.entries(pv) as [k, v] (k)}
              <tr>
                <td>{k}</td>
                <td><input value={JSON.stringify(v)} onchange={(e) => saveVar(k, (e.target as HTMLInputElement).value, pid)} /></td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/each}
    {/if}
  </section>

  <section>
    <h2>About</h2>
    <p>Puzzlebox is open source under the Apache License 2.0.</p>
    <p class="muted">
      The editor is built on <a href="https://github.com/google/blockly" target="_blank" rel="noopener">Blockly</a> by Google.
      Many editor features are modelled on
      <a href="https://turbowarp.org" target="_blank" rel="noopener">TurboWarp</a>,
      <a href="https://scratchaddons.com" target="_blank" rel="noopener">Scratch Addons</a> and
      <a href="https://penguinmod.com" target="_blank" rel="noopener">PenguinMod</a>; Puzzlebox writes its own versions and contains none of their code. Entity
      state lists come from the <a href="https://github.com/home-assistant/frontend" target="_blank" rel="noopener">Home Assistant frontend</a>.
      Puzzlebox isn't affiliated with any of these projects.
    </p>
  </section>
</div>

<style>
  .page {
    max-width: 860px;
    margin: 0 auto;
    padding: 16px;
    width: 100%;
    box-sizing: border-box;
    overflow: auto;
  }
  section {
    background: var(--pb-card);
    border: 1px solid var(--pb-divider);
    border-radius: 12px;
    padding: 8px 20px 16px;
    margin-bottom: 16px;
  }
  h2 {
    font-weight: 400;
  }
  h3 {
    font-size: 13px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--pb-text-2);
    margin: 16px 0 6px;
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .check {
    display: flex;
    gap: 8px;
    align-items: center;
    margin: 6px 0;
  }
  kbd {
    font: inherit;
    font-size: 12px;
    padding: 0 4px;
    border: 1px solid var(--pb-divider);
    border-radius: 4px;
  }
  .row {
    display: flex;
    gap: 8px;
    align-items: center;
    flex-wrap: wrap;
  }
  .token {
    padding: 8px 10px;
    background: color-mix(in srgb, var(--pb-text) 6%, transparent);
    border-radius: 8px;
    word-break: break-all;
  }
  .card {
    padding: 10px 12px;
    border: 1px solid var(--pb-divider);
    border-radius: 8px;
    margin-bottom: 8px;
  }
  .card p {
    margin: 4px 0 0;
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
  .warn {
    padding: 4px 14px;
    margin: 8px 0;
    border-radius: 8px;
    font-size: 14px;
    background: color-mix(in srgb, #ffa600 14%, transparent);
  }
  .warn ul {
    margin: 4px 0;
    padding-left: 20px;
  }
  .message {
    color: var(--pb-primary);
  }
  table {
    width: 100%;
    border-collapse: collapse;
  }
  td {
    padding: 4px;
  }
  td:first-child {
    width: 30%;
    font-weight: 500;
  }
  td input {
    width: 100%;
    box-sizing: border-box;
  }
  .link {
    background: none;
    border: 0;
    color: var(--pb-primary);
    cursor: pointer;
    font: inherit;
  }
  a {
    color: var(--pb-primary);
  }
</style>
