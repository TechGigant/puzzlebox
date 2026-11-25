<script lang="ts">
  import type { ProgramStatus } from './api';

  let { status }: { status: ProgramStatus | null | undefined } = $props();

  const LABELS: Record<string, string> = {
    disabled: 'Disabled',
    invalid: 'Has errors',
    error: 'Error',
    idle: 'Active',
    running: 'Running',
  };
  const state = $derived(status?.state ?? 'idle');
</script>

<span class="chip {state}" title={status?.last_error ?? ''}>
  <span class="dot"></span>{LABELS[state] ?? state}{#if state === 'running' && (status?.active_runs ?? 0) > 1}
    ×{status?.active_runs}{/if}
</span>

<style>
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 4px 10px;
    border-radius: 14px;
    font-size: 12px;
    white-space: nowrap;
    color: var(--pb-text-2);
    background: color-mix(in srgb, var(--pb-text) 8%, transparent);
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: #9e9e9e;
  }
  .idle .dot {
    background: #43a047;
  }
  .running .dot {
    background: var(--pb-primary);
    animation: pulse 1s infinite;
  }
  .invalid .dot,
  .error .dot {
    background: #e53935;
  }
  @keyframes pulse {
    50% {
      opacity: 0.3;
    }
  }
</style>
