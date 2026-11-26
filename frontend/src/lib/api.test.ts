import { describe, expect, it } from 'vitest';
import { updateNotice, type Status } from './api';

const status = (bridge: Status['bridge']): Status => ({ version: '0.3.0', ha_connected: true, ha_version: '2026.11.3', bridge });

describe('updateNotice', () => {
  it('is quiet when versions match or nothing is connected', () => {
    expect(updateNotice(status({ connected: true, integration_version: '0.3.0', compatible: true, outdated: null }))).toBeNull();
    expect(updateNotice(status({ connected: false }))).toBeNull();
  });

  it('asks to update the older side', () => {
    expect(updateNotice(status({ connected: true, integration_version: null, compatible: true, outdated: 'integration' }))).toContain(
      'from before 0.3.0',
    );
    expect(updateNotice(status({ connected: true, integration_version: '0.4.0', compatible: true, outdated: 'app' }))).toContain(
      'Update the app',
    );
    expect(updateNotice(status({ connected: true, integration_version: '0.1.0', compatible: false, outdated: 'integration' }))).toContain(
      'too old',
    );
  });
});
