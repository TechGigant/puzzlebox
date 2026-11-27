export interface BridgeStatus {
  connected: boolean;
  integration_version?: string | null;
  protocol?: number;
  compatible?: boolean;
  /** Which side should be updated, if the versions differ. */
  outdated?: 'app' | 'integration' | null;
  /** App features the integration lacks or has only in an older version. */
  missing?: string[];
}

export interface Status {
  version: string;
  ha_connected: boolean;
  ha_version: string | null;
  bridge?: BridgeStatus;
}

/** A sentence asking to update the older side, or null when versions match. */
export function updateNotice(status: Status | null): string | null {
  const b = status?.bridge;
  if (!status || !b?.connected || !b.outdated) return null;
  const integration = b.integration_version ? `(${b.integration_version})` : '(from before 0.3.0)';
  if (b.outdated === 'integration') {
    return b.compatible
      ? `The Puzzlebox integration ${integration} is older than this app (${status.version}). Update it in HACS and restart Home Assistant to use every feature.`
      : `The Puzzlebox integration ${integration} is too old for this app (${status.version}); programs don't appear in Home Assistant until you update it in HACS.`;
  }
  return b.compatible
    ? `This app (${status.version}) is older than the Puzzlebox integration ${integration}. Update the app under Settings → Apps to use every feature.`
    : `This app (${status.version}) is too old for the Puzzlebox integration ${integration}. Update the app under Settings → Apps.`;
}

export interface CompileError {
  block_id: string | null;
  message: string;
  /** Editor-only: shown in Problems, but the program still runs. */
  warning?: boolean;
}

export interface ProgramStatus {
  state: 'disabled' | 'invalid' | 'error' | 'idle' | 'running';
  active_runs: number;
  last_run: number | null;
  last_error: string | null;
  errors: CompileError[];
}

export type RunMode = 'single' | 'restart' | 'queued' | 'parallel';

export interface Program {
  id: string;
  name: string;
  description: string;
  enabled: boolean;
  mode: RunMode;
  max_runs: number;
  workspace?: object;
  /** "" = no folder; "/" separates nested folders. */
  folder: string;
  created_at: number;
  updated_at: number;
  status: ProgramStatus | null;
  errors?: CompileError[];
}

export interface ProgramVersion {
  id: number;
  saved_at: number;
  name: string;
  workspace?: object;
}

export interface RunRecord {
  id: string;
  program_id: string;
  started_at: number;
  finished_at: number | null;
  status: 'ok' | 'error' | 'cancelled' | 'running';
  error: string | null;
  trigger: unknown;
  trace?: {
    steps: { block: string; t: number; error?: string }[];
    truncated: boolean;
    variables: Record<string, unknown>;
    error_block: string | null;
  };
}

export type EngineEvent =
  | { type: 'status'; status: Status }
  | { type: 'statuses'; statuses: Record<string, ProgramStatus> }
  | { type: 'status'; program_id: string; status: ProgramStatus }
  | { type: 'run_started'; program_id: string; run_id: string; trigger: unknown }
  | { type: 'run_finished'; program_id: string; run_id: string; status: string; error: string | null; block_id: string | null }
  | { type: 'log'; program_id: string; run_id: string; level: string; message: string; ts: number }
  | { type: 'step'; program_id: string; run_id: string; block_id: string }
  | { type: 'paused'; program_id: string; run_id: string; block_id: string; variables: Record<string, unknown> }
  | { type: 'resumed'; program_id: string; run_id: string }
  | { type: 'removed'; program_id: string }
  | { type: 'registry_changed' }
  | { type: 'lagged'; missed: number };

/** Relative to the document so it works under the Ingress path prefix. */
export function apiUrl(path: string, ws = false): string {
  const url = new URL(path, document.baseURI);
  if (ws) url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
  return url.toString();
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const res = await fetch(apiUrl(path), {
    method,
    headers: body === undefined ? undefined : { 'Content-Type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error((data as { error?: string }).error ?? `${res.status} ${res.statusText}`);
  return data as T;
}

export const getJson = <T>(path: string) => request<T>('GET', path);

export const api = {
  programs: () => request<Program[]>('GET', 'api/programs'),
  program: (id: string) => request<Program>('GET', `api/programs/${id}`),
  create: (p: Partial<Program>) => request<Program>('POST', 'api/programs', p),
  update: (id: string, p: Partial<Program>) => request<Program>('PUT', `api/programs/${id}`, p),
  remove: (id: string) => request<{ ok: boolean }>('DELETE', `api/programs/${id}`),
  run: (id: string, handler?: number) => request<{ ok: boolean }>('POST', `api/programs/${id}/run`, { handler }),
  versions: (id: string) => request<ProgramVersion[]>('GET', `api/programs/${id}/versions`),
  version: (id: string, version: number) => request<ProgramVersion>('GET', `api/programs/${id}/versions/${version}`),
  runStack: (id: string, block: string) => request<{ ok: boolean }>('POST', `api/programs/${id}/run`, { block }),
  editorData: <T = unknown>(key: 'backpack' | 'favourites' | 'recent') => request<T[]>('GET', `api/editor/${key}`),
  setEditorData: (key: 'backpack' | 'favourites' | 'recent', contents: unknown[]) =>
    request<{ ok: boolean }>('PUT', `api/editor/${key}`, contents),
  cancel: (id: string) => request<{ ok: boolean }>('POST', `api/programs/${id}/cancel`, {}),
  setEnabled: (id: string, enabled: boolean) => request<Program>('POST', `api/programs/${id}/enabled`, { enabled }),
  runs: (id: string) => request<RunRecord[]>('GET', `api/programs/${id}/runs`),
  runDetail: (id: string) => request<RunRecord>('GET', `api/runs/${id}`),
  compile: (workspace: object) => request<{ errors: CompileError[] }>('POST', 'api/compile', { workspace }),
  importYaml: (yaml: string) => request<ImportedProgram[]>('POST', 'api/import', { yaml }),
  automations: () => request<{ entity_id: string; name: string; state: string }[]>('GET', 'api/import/automations'),
  importAutomations: (entity_ids: string[]) =>
    request<(ImportedProgram & { entity_id: string })[]>('POST', 'api/import/automations', { entity_ids }),
  settings: () => request<Settings>('GET', 'api/settings'),
  plugins: () => request<PluginsInfo>('GET', 'api/plugins'),
  reloadPlugins: () => request<{ ok: boolean; errors: { path: string; error: string }[] }>('POST', 'api/plugins/reload', {}),
  variables: () => request<{ global: Record<string, unknown>; programs: Record<string, Record<string, unknown>> }>('GET', 'api/variables'),
  setVariable: (name: string, value: unknown, program_id?: string) =>
    request<{ ok: boolean }>('PUT', 'api/variables', { name, value, program_id }),
};

export interface ImportedProgram {
  name: string;
  description: string;
  mode: RunMode;
  max_runs: number;
  workspace: object;
  raw_blocks: number;
  /** Things to check, such as `!secret` placeholders or an expanded blueprint. */
  warnings?: string[];
  error?: string;
}

export interface Settings {
  bridge_token: string;
  bridge_connected: boolean;
  bridge: BridgeStatus;
  mqtt_enabled: boolean;
  supervised: boolean;
  webhooks: string[];
}

export interface PluginsInfo {
  wasm: {
    id: string;
    name: string;
    version: string;
    description: string;
    blocks: number;
    capabilities: { http: string[]; services: string[]; states: boolean; kv: boolean };
    path: string;
  }[];
  python: string[];
  wasm_dirs: string[];
  packs_dirs: string[];
}

type Listener = (ev: EngineEvent) => void;

/** One shared, auto-reconnecting socket for status, engine events and debugger commands. */
class EventSocket {
  private socket: WebSocket | null = null;
  private listeners = new Set<Listener>();
  private delay = 500;
  private queue: string[] = [];
  /** Commands re-sent after every reconnect (e.g. debugger watches). */
  private sticky = new Map<string, string>();
  connected = false;

  start() {
    if (this.socket) return;
    this.open();
  }

  private open() {
    const s = new WebSocket(apiUrl('api/ws', true));
    this.socket = s;
    s.onopen = () => {
      this.delay = 500;
      this.connected = true;
      for (const m of this.sticky.values()) s.send(m);
      for (const m of this.queue.splice(0)) s.send(m);
    };
    s.onmessage = (ev) => {
      const msg = JSON.parse(ev.data) as EngineEvent;
      for (const l of this.listeners) l(msg);
    };
    s.onclose = () => {
      this.connected = false;
      this.socket = null;
      for (const l of this.listeners) l({ type: 'status', status: null as unknown as Status });
      setTimeout(() => this.open(), this.delay);
      this.delay = Math.min(this.delay * 2, 10_000);
    };
  }

  on(l: Listener): () => void {
    this.listeners.add(l);
    return () => this.listeners.delete(l);
  }

  send(msg: object, stickyKey?: string) {
    const text = JSON.stringify(msg);
    if (stickyKey) this.sticky.set(stickyKey, text);
    if (this.socket?.readyState === WebSocket.OPEN) this.socket.send(text);
    else if (!stickyKey) this.queue.push(text);
  }

  unstick(key: string) {
    this.sticky.delete(key);
  }
}

export const events = new EventSocket();
