/**
 * Times and numbers as the user set them in their Home Assistant profile (language, 12/24 hour
 * clock, number format, local or server time zone). Ingress runs us in a same-origin iframe, so
 * the parent's `hass` object is readable; outside HA the browser's settings apply.
 */

export interface HaLocale {
  language: string;
  /** "language" (what the language uses), "system" (the browser's), "12" or "24". */
  time_format: string;
  /** "language", "system", "comma_decimal", "decimal_comma", "space_comma" or "none". */
  number_format: string;
  /** "local" (the browser's) or "server" (Home Assistant's). */
  time_zone: string;
  /** Home Assistant's time zone, used when `time_zone` is "server". */
  server_time_zone?: string;
}

interface HassLike {
  locale?: Partial<HaLocale>;
  language?: string;
  config?: { time_zone?: string };
}

let cached: HaLocale | null = null;

function fromParent(): HassLike | null {
  try {
    if (window.parent === window) return null;
    const el = window.parent.document.querySelector('home-assistant') as (Element & { hass?: HassLike }) | null;
    return el?.hass ?? null;
  } catch {
    return null;
  }
}

export function haLocale(): HaLocale {
  if (cached) return cached;
  const hass = typeof window === 'undefined' ? null : fromParent();
  const l = hass?.locale ?? {};
  cached = {
    language: l.language || hass?.language || (typeof navigator === 'undefined' ? 'en' : navigator.language),
    time_format: l.time_format ?? 'language',
    number_format: l.number_format ?? 'language',
    time_zone: l.time_zone ?? 'local',
    server_time_zone: hass?.config?.time_zone,
  };
  return cached;
}

/** For tests, and when the profile may have changed. */
export function setHaLocale(l: HaLocale | null): void {
  cached = l;
}

const NUMBER_LOCALES: Record<string, string> = {
  comma_decimal: 'en-US',
  decimal_comma: 'de',
  space_comma: 'fr',
};

function numberLocale(l: HaLocale): string | undefined {
  if (l.number_format === 'system') return undefined;
  return NUMBER_LOCALES[l.number_format] ?? l.language;
}

function timeOptions(l: HaLocale): Intl.DateTimeFormatOptions {
  const o: Intl.DateTimeFormatOptions = {};
  if (l.time_format === '12') o.hour12 = true;
  // h23 rather than hour12: false, which some browsers render as "24:05" after midnight.
  if (l.time_format === '24') o.hourCycle = 'h23';
  if (l.time_zone === 'server' && l.server_time_zone) o.timeZone = l.server_time_zone;
  return o;
}

function timeLocale(l: HaLocale): string | undefined {
  return l.time_format === 'system' ? undefined : l.language;
}

const fmt = (ms: number, o: Intl.DateTimeFormatOptions, l = haLocale()) => {
  try {
    return new Intl.DateTimeFormat(timeLocale(l), { ...timeOptions(l), ...o }).format(ms);
  } catch {
    return new Date(ms).toLocaleString();
  }
};

/** "14:05:09" or "2:05:09 PM". */
export const formatTime = (ms: number, l?: HaLocale) => fmt(ms, { hour: 'numeric', minute: '2-digit', second: '2-digit' }, l);

/** "27 Nov 2026, 14:05". */
export const formatDateTime = (ms: number, l?: HaLocale) =>
  fmt(ms, { year: 'numeric', month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' }, l);

export const formatDate = (ms: number, l?: HaLocale) => fmt(ms, { year: 'numeric', month: 'short', day: 'numeric' }, l);

export function formatNumber(n: number, l = haLocale(), o: Intl.NumberFormatOptions = {}): string {
  try {
    return new Intl.NumberFormat(numberLocale(l), { maximumFractionDigits: 3, ...(l.number_format === 'none' ? { useGrouping: false } : {}), ...o }).format(n);
  } catch {
    return String(n);
  }
}
