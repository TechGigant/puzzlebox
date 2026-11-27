import { describe, expect, it } from 'vitest';
import { formatDateTime, formatNumber, formatTime, type HaLocale } from './locale';

const base: HaLocale = { language: 'en-GB', time_format: 'language', number_format: 'language', time_zone: 'server', server_time_zone: 'UTC' };
const at = Date.UTC(2026, 10, 27, 14, 5, 9);

describe('Home Assistant locale', () => {
  it('follows the 12/24 hour setting', () => {
    expect(formatTime(at, base)).toBe('14:05:09');
    expect(formatTime(at, { ...base, time_format: '12' })).toMatch(/^2:05:09\spm$/i);
    expect(formatTime(at, { ...base, language: 'en-US', time_format: '24' })).toBe('14:05:09');
    expect(formatTime(at, { ...base, language: 'en-US' })).toMatch(/PM/);
  });

  it('uses the server time zone when the profile says so', () => {
    expect(formatTime(at, { ...base, server_time_zone: 'Europe/Amsterdam' })).toBe('15:05:09');
    expect(formatDateTime(at, base)).toBe('27 Nov 2026, 14:05');
  });

  it('follows the number format', () => {
    expect(formatNumber(1234.5, { ...base, number_format: 'decimal_comma' })).toBe('1.234,5');
    expect(formatNumber(1234.5, { ...base, number_format: 'comma_decimal' })).toBe('1,234.5');
    expect(formatNumber(1234.5, { ...base, number_format: 'none' })).toBe('1234.5');
  });
});
