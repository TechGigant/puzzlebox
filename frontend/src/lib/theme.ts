import * as Blockly from 'blockly/core';

export interface HaTokens {
  primary: string;
  background: string;
  card: string;
  text: string;
  secondaryText: string;
  divider: string;
  dark: boolean;
}

const LIGHT: HaTokens = {
  primary: '#03a9f4',
  background: '#fafafa',
  card: '#ffffff',
  text: '#212121',
  secondaryText: '#727272',
  divider: '#e0e0e0',
  dark: false,
};

const DARK: HaTokens = {
  primary: '#03a9f4',
  background: '#111111',
  card: '#1c1c1c',
  text: '#e1e1e1',
  secondaryText: '#9b9b9b',
  divider: '#2f2f2f',
  dark: true,
};

/** Category hues, tuned to sit well on both HA light and dark backgrounds. */
const CATEGORIES: Record<string, { light: string; dark: string }> = {
  logic: { light: '#5c6bc0', dark: '#7986cb' },
  // Loops live in Control, so they share its colour.
  loop: { light: '#546e7a', dark: '#78909c' },
  math: { light: '#1e88e5', dark: '#42a5f5' },
  text: { light: '#8e24aa', dark: '#ab47bc' },
  list: { light: '#f4511e', dark: '#ff7043' },
  variable: { light: '#e53975', dark: '#ec5f91' },
  procedure: { light: '#6d4c41', dark: '#8d6e63' },
  trigger: { light: '#fb8c00', dark: '#ffa726' },
  condition: { light: '#00897b', dark: '#26a69a' },
  action: { light: '#039be5', dark: '#29b6f6' },
  time: { light: '#7cb342', dark: '#9ccc65' },
  flow: { light: '#546e7a', dark: '#78909c' },
  dict: { light: '#5e35b1', dark: '#9575cd' },
  http: { light: '#455a64', dark: '#90a4ae' },
  stored: { light: '#c2185b', dark: '#f06292' },
  plugin: { light: '#6a1b9a', dark: '#ba68c8' },
  favourite: { light: '#f9a825', dark: '#fdd835' },
  section: { light: '#9e9e9e', dark: '#616161' },
  colour: { light: '#00acc1', dark: '#4dd0e1' },
  home: { light: '#0288d1', dark: '#4fc3f7' },
};

/** Stable colour for runtime-contributed categories (HA domains, plugins). */
export function hashColour(name: string, dark: boolean): string {
  let h = 0;
  for (const c of name) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  const hue = h % 360;
  const [s, l] = dark ? [55, 62] : [60, 45];
  const a = (s / 100) * Math.min(l / 100, 1 - l / 100);
  const f = (n: number) => {
    const k = (n + hue / 30) % 12;
    const c = l / 100 - a * Math.max(-1, Math.min(k - 3, 9 - k, 1));
    return Math.round(c * 255).toString(16).padStart(2, '0');
  };
  return `#${f(0)}${f(8)}${f(4)}`;
}

export function parseHex(hex: string): [number, number, number] | null {
  const m = hex.trim().match(/^#([0-9a-f]{3}|[0-9a-f]{6})$/i);
  if (!m) {
    const rgb = hex.match(/rgba?\(\s*(\d+)[,\s]+(\d+)[,\s]+(\d+)/i);
    return rgb ? [Number(rgb[1]), Number(rgb[2]), Number(rgb[3])] : null;
  }
  const h = m[1].length === 3 ? [...m[1]].map((c) => c + c).join('') : m[1];
  return [0, 2, 4].map((i) => parseInt(h.slice(i, i + 2), 16)) as [number, number, number];
}

export function luminance(color: string): number {
  const rgb = parseHex(color);
  if (!rgb) return 1;
  const [r, g, b] = rgb.map((v) => {
    const c = v / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** Mixes `a` toward `b` by `t` (0..1); returns hex. Blockly needs concrete colours, not CSS vars. */
export function mix(a: string, b: string, t: number): string {
  const ca = parseHex(a) ?? [0, 0, 0];
  const cb = parseHex(b) ?? [0, 0, 0];
  return (
    '#' +
    ca
      .map((v, i) => Math.round(v + (cb[i] - v) * t))
      .map((v) => v.toString(16).padStart(2, '0'))
      .join('')
  );
}

/**
 * Ingress runs us in a same-origin iframe inside HA, so the parent's theme
 * variables are readable. Outside HA (dev) fall back to the OS colour scheme.
 */
export function readHaTokens(): HaTokens {
  const prefersDark = window.matchMedia?.('(prefers-color-scheme: dark)').matches ?? false;
  const base = prefersDark ? DARK : LIGHT;
  let style: CSSStyleDeclaration | null = null;
  try {
    if (window.parent !== window) style = getComputedStyle(window.parent.document.documentElement);
  } catch {
    style = null;
  }
  if (!style) return base;
  const get = (name: string, fallback: string) => style!.getPropertyValue(name).trim() || fallback;
  const background = get('--primary-background-color', base.background);
  return {
    primary: get('--primary-color', base.primary),
    background,
    card: get('--card-background-color', base.card),
    text: get('--primary-text-color', base.text),
    secondaryText: get('--secondary-text-color', base.secondaryText),
    divider: get('--divider-color', base.divider),
    dark: luminance(background) < 0.2,
  };
}

export function buildTheme(t: HaTokens, extraStyles: string[] = []): Blockly.Theme {
  const blockStyles: Record<string, Blockly.Theme.BlockStyle> = {};
  const categoryStyles: Record<string, Blockly.Theme.CategoryStyle> = {};
  const all: [string, string][] = [
    ...Object.entries(CATEGORIES).map(([n, c]) => [n, t.dark ? c.dark : c.light] as [string, string]),
    ...extraStyles.filter((n) => !(n in CATEGORIES)).map((n) => [n, hashColour(n, t.dark)] as [string, string]),
  ];
  for (const [name, colour] of all) {
    blockStyles[`${name}_blocks`] = {
      colourPrimary: colour,
      colourSecondary: mix(colour, t.card, 0.35),
      colourTertiary: mix(colour, '#000000', 0.25),
      hat: '',
    };
    // Second shade for nested blocks of the same colour (zebra striping).
    const alt = mix(colour, t.dark ? '#ffffff' : '#000000', t.dark ? 0.18 : 0.16);
    blockStyles[`${name}_blocks_alt`] = {
      colourPrimary: alt,
      colourSecondary: mix(alt, t.card, 0.35),
      colourTertiary: mix(alt, '#000000', 0.25),
      hat: '',
    };
    categoryStyles[`${name}_category`] = { colour };
  }
  // Entry blocks ("when …") get a hat, like Scratch's event blocks.
  blockStyles.entry_blocks = { ...blockStyles.trigger_blocks, hat: 'cap' };

  return Blockly.Theme.defineTheme(t.dark ? 'ha-dark' : 'ha-light', {
    name: t.dark ? 'ha-dark' : 'ha-light',
    base: Blockly.Themes.Classic,
    blockStyles,
    categoryStyles,
    componentStyles: {
      workspaceBackgroundColour: t.background,
      toolboxBackgroundColour: t.card,
      toolboxForegroundColour: t.text,
      flyoutBackgroundColour: mix(t.card, t.background, 0.5),
      flyoutForegroundColour: t.text,
      flyoutOpacity: 0.97,
      scrollbarColour: t.secondaryText,
      scrollbarOpacity: 0.35,
      insertionMarkerColour: t.primary,
      insertionMarkerOpacity: 0.4,
      markerColour: t.primary,
      cursorColour: t.primary,
      selectedGlowColour: t.primary,
      selectedGlowOpacity: 0.6,
    },
    fontStyle: { family: 'Roboto, "Noto Sans", system-ui, sans-serif', weight: '500', size: 11 },
    startHats: false,
  });
}

/** Mirrors the tokens onto our own document so the app chrome matches the workspace. */
export function applyCssTokens(t: HaTokens): void {
  const root = document.documentElement.style;
  root.setProperty('--pb-primary', t.primary);
  root.setProperty('--pb-bg', t.background);
  root.setProperty('--pb-card', t.card);
  root.setProperty('--pb-text', t.text);
  root.setProperty('--pb-text-2', t.secondaryText);
  root.setProperty('--pb-divider', t.divider);
  root.colorScheme = t.dark ? 'dark' : 'light';
}

/** Calls `cb` whenever HA's theme or the OS scheme changes. Returns an unsubscribe fn. */
export function watchTheme(cb: (t: HaTokens) => void): () => void {
  const fire = () => cb(readHaTokens());
  const mq = window.matchMedia?.('(prefers-color-scheme: dark)');
  mq?.addEventListener('change', fire);
  let observer: MutationObserver | null = null;
  try {
    if (window.parent !== window) {
      observer = new MutationObserver(fire);
      observer.observe(window.parent.document.documentElement, {
        attributes: true,
        attributeFilter: ['style', 'class'],
      });
    }
  } catch {
    observer = null;
  }
  return () => {
    mq?.removeEventListener('change', fire);
    observer?.disconnect();
  };
}
