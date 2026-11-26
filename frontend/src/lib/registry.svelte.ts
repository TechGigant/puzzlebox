import { getJson } from './api';
import { installRegistry, type BlockDef } from './blocks/dynamic';
import { devicesCategory } from './blocks/devices';
import { loadRegistry as loadHaRegistry, type HaRegistry } from './blocks/field_entity';
import type { ExtraCategory } from './toolbox';

/** Runtime block registry (autogen + plugins), filled from the server. */
export const registry = $state<{
  categories: ExtraCategory[];
  styles: string[];
  loaded: boolean;
  packs: { id: string; name: string; version: string; description: string; blocks: number }[];
  packErrors: { path: string; error: string }[];
  ha: HaRegistry | null;
}>({ categories: [], styles: [], loaded: false, packs: [], packErrors: [], ha: null });

export async function loadRegistry(): Promise<void> {
  const haReg = loadHaRegistry(true);
  const res = await getJson<{
    blocks: BlockDef[];
    packs: typeof registry.packs;
    pack_errors: typeof registry.packErrors;
  }>('api/registry');
  const { categories, styles } = installRegistry(res.blocks);
  const ha = await haReg;
  const devices = devicesCategory(ha);
  registry.ha = ha;
  registry.categories = devices ? [devices, ...categories] : categories;
  registry.styles = styles;
  registry.packs = res.packs;
  registry.packErrors = res.pack_errors;
  registry.loaded = true;
}
