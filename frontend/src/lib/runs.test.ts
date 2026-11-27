import { describe, expect, it } from 'vitest';
import { activity, blockIds, fmtDuration, triggerSummary } from './runs';

describe('runs', () => {
  it('formats durations', () => {
    expect(fmtDuration(22_400)).toBe('22 s');
    expect(fmtDuration(250_000)).toBe('4 min 10 s');
    expect(fmtDuration(3_900_000)).toBe('1 h 5 min');
  });

  it('says what a run is doing', () => {
    const run = { run_id: 'r', program_id: 'p', started_at: 0, summary: '', block_id: 'w', waiting: null };
    expect(activity({ ...run, waiting: { block_id: 'w', text: 'waiting 30 s', until: 22_000 } }, 0, () => null)).toBe(
      'waiting 30 s, 22 s left',
    );
    expect(activity(run, 0, () => 'turn on Hall')).toBe('at “turn on Hall”');
  });

  it('describes old runs without a summary', () => {
    expect(triggerSummary({ trigger: { platform: 'manual' } })).toBe('run manually');
    expect(
      triggerSummary({ trigger: { platform: 'state', to_state: { state: 'on', attributes: { friendly_name: 'Motion' } } } }),
    ).toBe('Motion turned on');
    expect(triggerSummary({ summary: 'given', trigger: null })).toBe('given');
  });

  it('collects block ids but not shadows', () => {
    const ws = {
      blocks: {
        blocks: [{ type: 'a', id: '1', inputs: { X: { shadow: { type: 's', id: 'sh' }, block: { type: 'b', id: '2' } } } }],
      },
    };
    expect(blockIds(ws)).toEqual(['1', '2']);
  });
});
