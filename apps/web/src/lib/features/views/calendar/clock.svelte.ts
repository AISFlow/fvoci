/**
 * The clock for "today", read again every minute while something reads it: a
 * page left open past midnight moves today on. Nothing ticks while no part of
 * the page asks for the time.
 */
import { createSubscriber } from 'svelte/reactivity';

export class MinuteClock {
  readonly #subscribe = createSubscriber((update) => {
    const timer = setInterval(update, 60_000);
    return () => {
      clearInterval(timer);
    };
  });

  /** The current moment, in milliseconds. */
  get now(): number {
    this.#subscribe();
    return Date.now();
  }
}
