/** What a shown view reads, read where an effect follows it, for the tests of what is followed. */
import { flushSync } from 'svelte';

/** Reads `read` in an effect, as a view that shows it does, until the function given back is called. */
export function watched(read: () => void): () => void {
  const stop = $effect.root(() => {
    $effect(() => {
      read();
    });
  });
  flushSync();
  return stop;
}
