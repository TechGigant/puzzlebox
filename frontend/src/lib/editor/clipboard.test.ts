// @vitest-environment jsdom
import * as Blockly from 'blockly/core';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { installSharedClipboard, storeCopy, storedCopy } from './clipboard';

const ws = (id: string) => ({ id, isFlyout: false }) as unknown as Blockly.WorkspaceSvg;
const data = (type: string) => ({ paster: 'block', blockState: { type, x: 10, y: 20 }, typeCounts: {} }) as unknown as Blockly.ICopyData;

afterEach(() => {
  vi.restoreAllMocks();
  localStorage.clear();
});

describe('shared clipboard', () => {
  it('keeps the copy in local storage, for other programs and tabs', () => {
    storeCopy(data('ha_log'), ws('a'));
    expect(JSON.parse(localStorage.getItem('puzzlebox.clipboard')!)).toEqual({ data: data('ha_log'), from: 'a' });
    expect(storedCopy()).toEqual({ data: data('ha_log'), from: 'a' });
  });

  it('works in this tab when storage is unavailable', () => {
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('quota');
    });
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
      throw new Error('blocked');
    });
    storeCopy(data('ha_wait'), ws('b'));
    expect(storedCopy()).toEqual({ data: data('ha_wait'), from: 'b' });
  });

  it('keeps Blockly’s copy, cut and paste shortcuts on their keys', () => {
    const registry = Blockly.ShortcutRegistry.registry;
    const names = Blockly.ShortcutItems.names;
    const keys = () => [names.COPY, names.CUT, names.PASTE].map((n) => registry.getKeyCodesByShortcutName(n));
    const before = keys();
    installSharedClipboard();
    expect(keys()).toEqual(before);
    expect(before.every((k) => k.length)).toBe(true);
  });
});
