import { describe, expect, it } from 'vitest';
import * as Blockly from 'blockly/core';
import { buildTheme, contrast, hashColour, luminance, mix, parseHex, type HaTokens } from './theme';

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

describe('block colours', () => {
  const tokens = (dark: boolean): HaTokens => ({
    primary: '#03a9f4',
    background: dark ? '#111111' : '#fafafa',
    card: dark ? '#1c1c1c' : '#ffffff',
    text: dark ? '#e1e1e1' : '#212121',
    secondaryText: '#727272',
    divider: '#e0e0e0',
    dark,
  });

  it('keep white labels readable (WCAG AA) in both themes, zebra shades included', () => {
    for (const dark of [false, true]) {
      const theme = buildTheme(tokens(dark), ['light', 'zigbee2mqtt', 'my_plugin']) as Blockly.Theme;
      // Blockly's own base styles give hues ("310"), for blocks we don't use.
      for (const [name, style] of Object.entries(theme.blockStyles).filter(([, s]) => s.colourPrimary.startsWith('#'))) {
        expect(contrast(style.colourPrimary, '#ffffff'), `${name} (${dark ? 'dark' : 'light'})`).toBeGreaterThanOrEqual(4.5);
      }
    }
  });

  it('gives runtime categories distinct, readable colours', () => {
    const names = ['light', 'switch', 'climate', 'media_player', 'z2m', 'esphome'];
    const colours = names.map((n) => hashColour(n));
    expect(new Set(colours).size).toBe(names.length);
    for (const c of colours) expect(contrast(c, '#ffffff')).toBeGreaterThanOrEqual(4.5);
  });
});
