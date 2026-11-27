import type { Zigbee2MqttStatus } from './api';

const plural = (n: number, one: string) => `${n} ${one}${n === 1 ? '' : 's'}`;

/** What Settings says about one Zigbee2MQTT base topic. */
export function z2mStatusText(s: Zigbee2MqttStatus): { text: string; ok: boolean; bad: boolean } {
  switch (s.state) {
    case 'ok':
      return { text: `${plural(s.devices ?? 0, 'device')}, ${plural(s.groups ?? 0, 'group')}`, ok: true, bad: false };
    case 'waiting':
      return { text: 'waiting for bridge/devices', ok: false, bad: false };
    case 'disconnected':
      return { text: 'waiting for the MQTT broker', ok: false, bad: false };
    default:
      return { text: 'not found (is the base topic right?)', ok: false, bad: true };
  }
}
