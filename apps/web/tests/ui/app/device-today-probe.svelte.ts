import { flushSync } from 'svelte';

import { DeviceToday } from '$lib/state/index.ts';

export function mountDeviceTodayProbe(initialZone: string): {
  readonly seen: string[];
  setZone: (next: string) => void;
  stop: () => void;
} {
  let zone = $state(initialZone);
  const clock = new DeviceToday(() => zone);
  const seen: string[] = [];

  const stop = $effect.root(() => {
    $effect(() => {
      seen.push(clock.date);
    });
  });
  flushSync();

  return {
    seen,
    setZone(next: string) {
      zone = next;
      flushSync();
    },
    stop,
  };
}
