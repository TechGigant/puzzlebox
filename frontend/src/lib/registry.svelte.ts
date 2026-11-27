import { getJson, type Status } from './api';
import { installRegistry, type BlockDef } from './blocks/dynamic';
import { devicesCategory } from './blocks/devices';
import { loadRegistry as loadHaRegistry, type HaRegistry } from './blocks/field_entity';
import type { Why } from './editor/unavailable';
import type { ExtraCategory } from './toolbox';

/** Runtime block registry (autogen + plugins), filled from the server. */
export const registry = $state<{
  categories: ExtraCategory[];
  styles: string[];
  loaded: boolean;
  /** Counts loads, so editors can show blocks that just became available. */
  version: number;
  packs: { id: string; name: string; version: string; description: string; blocks: number }[];
  packErrors: { path: string; error: string }[];
  ha: HaRegistry | null;
  /** Where the blocks came from: sources, and those only shown from the server's store. */
  sources: string[];
  cached: string[];
  /** The Home Assistant blocks were built from HA since it last connected. */
  haCurrent: boolean;
  /** The app's connection status, kept here to explain missing blocks. */
  status: Status | null;
}>({
  categories: [],
  styles: [],
  loaded: false,
  version: 0,
  packs: [],
  packErrors: [],
  ha: null,
  sources: [],
  cached: [],
  haCurrent: false,
  status: null,
});

export async function loadRegistry(): Promise<void> {
  const haReg = loadHaRegistry(true);
  const res = await getJson<{
    blocks: BlockDef[];
    packs: typeof registry.packs;
    pack_errors: typeof registry.packErrors;
    cached?: string[];
    ha_current?: boolean;
  }>('api/registry');
  const { categories, styles } = installRegistry(res.blocks);
  const ha = await haReg;
  const devices = devicesCategory(ha);
  registry.ha = ha;
  registry.categories = devices ? [devices, ...categories] : categories;
  registry.styles = styles;
  registry.packs = res.packs;
  registry.packErrors = res.pack_errors;
  registry.sources = [...new Set(res.blocks.map((b) => b.source))];
  registry.cached = res.cached ?? [];
  registry.haCurrent = res.ha_current ?? true;
  registry.loaded = true;
  registry.version++;
}

const HA_PREFIXES = ['ha_svc__', 'ha_trg__', 'ha_cnd__'];

/** Why the editor has no block of this type, and whether it may still turn up. */
export function whyMissing(type: string): Why {
  const st = registry.status;
  const live = (prefix: string) => registry.sources.some((s) => s.startsWith(prefix) && !registry.cached.includes(s));
  if (HA_PREFIXES.some((p) => type.startsWith(p))) {
    if (!st?.ha_connected) return { reason: "Home Assistant isn't connected, so its blocks can't be shown.", pending: true };
    if (!registry.haCurrent) return { reason: "Home Assistant's blocks are still loading.", pending: true };
    return { reason: "Home Assistant doesn't offer this any more: the integration that provided it may have been removed, renamed or failed to start.", pending: false };
  }
  if (type.startsWith('z2m__')) {
    if (!live('z2m:')) return { reason: "Zigbee2MQTT hasn't sent its devices yet (is it running, and is MQTT connected?).", pending: true };
    return { reason: "Zigbee2MQTT doesn't have this device or group any more.", pending: false };
  }
  if (type.startsWith('py__')) {
    if (!st?.ha_connected || !st.bridge?.connected)
      return { reason: "The Python plugin that provides it hasn't connected yet (it runs in Home Assistant).", pending: true };
    return { reason: 'The Python plugin that provided it is gone, or no longer has this block.', pending: false };
  }
  if (type.startsWith('wasm__')) return { reason: "The plugin that provided it isn't installed any more.", pending: false };
  if (type.startsWith('pack__')) return { reason: "The block pack that provided it isn't installed any more.", pending: false };
  return { reason: "This block isn't part of this version of Puzzlebox.", pending: false };
}
