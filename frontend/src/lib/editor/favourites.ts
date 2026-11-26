//! "Favourites" toolbox category: pinned blocks plus the most recently added ones. Both lists
//! are kept by the app, so they follow you between programs and browsers.

import * as Blockly from 'blockly/core';
import { api } from '../api';

type State = Blockly.serialization.blocks.State;
type FlyoutItem = Blockly.utils.toolbox.FlyoutItemInfo;

export const FAVOURITES_CATEGORY = 'PB_FAVOURITES';
export const FAVOURITES_NAME = 'Favourites';
const RECENT_MAX = 12;

let favourites: State[] = [];
let recent: State[] = [];
let loaded: Promise<void> | null = null;

export function loadFavourites(): Promise<void> {
  loaded ??= Promise.all([api.editorData<State>('favourites'), api.editorData<State>('recent')])
    .then(([f, r]) => {
      favourites = f;
      recent = r;
    })
    .catch(() => {});
  return loaded;
}

const timers: Record<string, ReturnType<typeof setTimeout>> = {};
function persist(key: 'favourites' | 'recent', list: State[]) {
  clearTimeout(timers[key]);
  timers[key] = setTimeout(() => api.setEditorData(key, list).catch(() => {}), 800);
}

/** The block as a reusable template: no ids, no position, no blocks after it. */
function template(block: Blockly.Block): State {
  const s = Blockly.serialization.blocks.save(block, { addNextBlocks: false, saveIds: false })!;
  delete s.x;
  delete s.y;
  return s;
}

const key = (s: State) => JSON.stringify(s);

function isFavourite(block: Blockly.Block) {
  const k = key(template(block));
  return favourites.some((f) => key(f) === k);
}

function toggleFavourite(block: Blockly.Block) {
  const t = template(block);
  const k = key(t);
  favourites = favourites.some((f) => key(f) === k) ? favourites.filter((f) => key(f) !== k) : [...favourites, t];
  persist('favourites', favourites);
  refresh(block.workspace as Blockly.WorkspaceSvg);
}

/** Re-renders the flyout if the Favourites category is open. */
function refresh(ws: Blockly.WorkspaceSvg) {
  const main = ws.isFlyout ? (ws.targetWorkspace ?? ws) : ws;
  const toolbox = main.getToolbox() as Blockly.Toolbox | null;
  const selected = toolbox?.getSelectedItem() as Blockly.ToolboxCategory | null;
  if (selected?.getName?.() === FAVOURITES_NAME) toolbox?.refreshSelection();
}

/** Remembers a block the user just added (from the toolbox, search or paste). */
export function noteUsed(block: Blockly.Block) {
  if (block.isShadow() || block.isInFlyout) return;
  // Recent blocks are remembered by type with their default contents.
  const t = template(block);
  recent = [t, ...recent.filter((r) => r.type !== t.type)].slice(0, RECENT_MAX);
  persist('recent', recent);
}

function blocks(list: State[]): FlyoutItem[] {
  return list.filter((s) => Blockly.Blocks[s.type]).map((s) => ({ kind: 'block', ...s }) as unknown as FlyoutItem);
}

export function registerFavourites(ws: Blockly.WorkspaceSvg) {
  loadFavourites().then(() => refresh(ws));
  ws.registerToolboxCategoryCallback(FAVOURITES_CATEGORY, () => {
    const label = (text: string) => ({ kind: 'label', text }) as FlyoutItem;
    const fav = blocks(favourites);
    const rec = blocks(recent);
    return [
      label('Favourites'),
      ...(fav.length ? fav : [label('Right-click any block → Add to favourites')]),
      label('Recently used'),
      ...(rec.length ? rec : [label('Blocks you add show up here')]),
    ];
  });
}

let menuRegistered = false;
export function registerFavouritesMenu() {
  if (menuRegistered) return;
  menuRegistered = true;
  Blockly.ContextMenuRegistry.registry.register({
    id: 'puzzlebox_favourite',
    scopeType: Blockly.ContextMenuRegistry.ScopeType.BLOCK,
    weight: 12,
    displayText: (scope) => (isFavourite(scope.block!) ? 'Remove from favourites' : 'Add to favourites'),
    preconditionFn: (scope) => (scope.block && !scope.block.isShadow() ? 'enabled' : 'hidden'),
    callback: (scope) => toggleFavourite(scope.block!),
  });
}
