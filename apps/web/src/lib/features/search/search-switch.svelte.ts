/**
 * Whether the search is there: unknown, and so off, until the backend says
 * (`searchEnabled`). A backend that cannot say is asked again a few times,
 * waiting longer each time (`RETRY_SECONDS`), and leaves it off meanwhile.
 */
import type { ContentSearch } from '$lib/state/content-search.ts';

/** How long to wait before asking again after each failure, in seconds. */
export const RETRY_SECONDS = [5, 15, 60, 300] as const;

export class SearchSwitch {
  on = $state(false);
  readonly #search: ContentSearch;

  constructor(search: ContentSearch) {
    this.#search = search;
    this.#ask(0);
  }

  #ask(failures: number): void {
    this.#search.enabled().then(
      (enabled) => {
        this.on = enabled;
      },
      () => {
        const wait = RETRY_SECONDS[failures];
        if (wait === undefined) return;
        setTimeout(() => {
          this.#ask(failures + 1);
        }, wait * 1000);
      },
    );
  }
}
