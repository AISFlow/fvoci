import { onMounted, onUnmounted, watch, type WatchSource } from "vue";

/**
 * Runs `effect` once the component is mounted and again after every render
 * in which one of `sources` changed, like a React effect with those
 * dependencies: its cleanup runs before the next run and on unmount, and it
 * runs after the DOM update, so template refs are in place. The attachment
 * viewers keep their React lifetimes this way (a download, a worker or a
 * page render belongs to exactly one run and is released by its cleanup).
 */
export function useEffectAfterRender(
  sources: WatchSource<unknown>[],
  effect: () => void | (() => void),
): void {
  let cleanup: void | (() => void);
  let mounted = false;
  const run = () => {
    if (cleanup) cleanup();
    cleanup = effect();
  };
  onMounted(() => {
    mounted = true;
    run();
  });
  watch(
    sources,
    () => {
      if (mounted) run();
    },
    { flush: "post" },
  );
  onUnmounted(() => {
    mounted = false;
    if (cleanup) cleanup();
    cleanup = undefined;
  });
}
