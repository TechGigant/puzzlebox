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

/**
 * Category colours. Block labels are white, so every colour (and its darker zebra shade) keeps
 * at least 4.5:1 contrast with white (WCAG AA), in light and dark themes alike.
 */
const CATEGORIES: Record<string, string> = {
  logic: '#5c6bc0',
  // Loops live in Control, so they share its colour.
  loop: '#546e7a',
  math: '#1976d2',
  text: '#8e24aa',
  list: '#c43e0e',
  variable: '#d81b60',
  procedure: '#6d4c41',
  trigger: '#b35c00',
  condition: '#00796b',
  action: '#0277bd',
  time: '#4e7d22',
  flow: '#546e7a',
  dict: '#5e35b1',
  http: '#455a64',
  stored: '#ad1457',
  plugin: '#6a1b9a',
  favourite: '#8f6a00',
  section: '#757575',
  colour: '#007c89',
  home: '#01579b',
};

/** WCAG contrast ratio of two colours (1 to 21). */
export function contrast(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

function hsl(hue: number, s: number, l: number): string {
  const a = (s / 100) * Math.min(l / 100, 1 - l / 100);
  const f = (n: number) => {
    const k = (n + hue / 30) % 12;
    const c = l / 100 - a * Math.max(-1, Math.min(k - 3, 9 - k, 1));
    return Math.round(c * 255).toString(16).padStart(2, '0');
  };
  return `#${f(0)}${f(8)}${f(4)}`;
}

/** Stable colour for runtime-contributed categories (HA domains, plugins), dark enough for white text. */
export function hashColour(name: string): string {
  let h = 0;
  for (const c of name) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  const hue = h % 360;
  let l = 45;
  while (l > 20 && contrast(hsl(hue, 60, l), '#ffffff') < 4.6) l -= 1;
  return hsl(hue, 60, l);
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
    ...Object.entries(CATEGORIES),
    ...extraStyles.filter((n) => !(n in CATEGORIES)).map((n) => [n, hashColour(n)] as [string, string]),
  ];
  for (const [name, colour] of all) {
    blockStyles[`${name}_blocks`] = {
      colourPrimary: colour,
      colourSecondary: mix(colour, t.card, 0.35),
      colourTertiary: mix(colour, '#000000', 0.25),
      hat: '',
    };
    // Second shade for nested blocks of the same colour (zebra striping). Darker in both
    // themes: a lighter shade would lose the label contrast.
    const alt = mix(colour, '#000000', 0.2);
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
  // Shadow blocks are a faded block colour: light on light themes, so their labels turn dark.
  root.setProperty('--pb-shadow-text', t.dark ? '#ffffff' : t.text);
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
