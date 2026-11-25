import { describe, expect, it } from 'vitest';
import { luminance, mix, parseHex } from './theme';

describe('colour helpers', () => {
  it('parses hex and rgb()', () => {
    expect(parseHex('#fff')).toEqual([255, 255, 255]);
    expect(parseHex('#03a9f4')).toEqual([3, 169, 244]);
    expect(parseHex('rgb(17, 17, 17)')).toEqual([17, 17, 17]);
    expect(parseHex('var(--x)')).toBeNull();
  });

  it('detects dark backgrounds', () => {
    expect(luminance('#111111')).toBeLessThan(0.2);
    expect(luminance('#fafafa')).toBeGreaterThan(0.2);
  });

  it('mixes colours', () => {
    expect(mix('#000000', '#ffffff', 0.5)).toBe('#808080');
    expect(mix('#ff0000', '#0000ff', 0)).toBe('#ff0000');
  });
});
