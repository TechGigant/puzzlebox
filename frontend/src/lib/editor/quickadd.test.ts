import { describe, expect, it } from 'vitest';
import { search, type Entry } from './quickadd';

const entry = (label: string, path = 'Home Assistant › Light'): Entry => ({
  label,
  path,
  state: { type: 'x' },
  haystack: `${label} ${path}`.toLowerCase(),
});

describe('quick add search', () => {
  const entries = [entry('Turn off light'), entry('Turn on light'), entry('Toggle light'), entry('wait 5 seconds', 'Actions')];

  it('needs every word', () => {
    expect(search(entries, 'turn off').map((e) => e.label)).toEqual(['Turn off light']);
    expect(search(entries, 'turn zebra')).toEqual([]);
  });

  it('ranks matches at the start of the label first', () => {
    expect(search(entries, 'light')[0].label).not.toBe('wait 5 seconds');
    expect(search(entries, 'wait')[0].label).toBe('wait 5 seconds');
    expect(search(entries, 'tog')[0].label).toBe('Toggle light');
  });
});
