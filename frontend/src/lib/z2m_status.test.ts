import { describe, expect, it } from 'vitest';
import { z2mStatusText } from './z2m_status';

describe('z2mStatusText', () => {
  it('counts devices and groups', () => {
    expect(z2mStatusText({ base: 'zigbee2mqtt', state: 'ok', devices: 14, groups: 1 }).text).toBe('14 devices, 1 group');
    expect(z2mStatusText({ base: 'zigbee2mqtt', state: 'ok', devices: 1, groups: null }).text).toBe('1 device, 0 groups');
  });

  it('tells waiting from missing', () => {
    expect(z2mStatusText({ base: 'z', state: 'waiting', devices: null, groups: null })).toMatchObject({ text: 'waiting for bridge/devices', bad: false });
    expect(z2mStatusText({ base: 'z', state: 'not_found', devices: null, groups: null }).bad).toBe(true);
  });
});
