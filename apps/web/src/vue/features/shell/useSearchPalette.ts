import { useQuery } from "@tanstack/vue-query";
import { computed, onScopeDispose, ref, toValue, watch, type MaybeRefOrGetter, type Ref } from "vue";
import type { SearchResult } from "@/features/workspace/search-target";
import { searchQuery } from "@/lib/queries";

type KeyTarget = Pick<EventTarget, "addEventListener" | "removeEventListener">;

/** How long typing pauses before the palette searches (the React palette's delay). */
export const SEARCH_DEBOUNCE_MS = 250;

/**
 * Ctrl+K or Cmd+K anywhere on the page opens the palette; Escape closes an
 * open one (features/workspace/search-command.tsx). Listens on `target`
 * (the window) until the calling scope ends.
 */
export function useSearchShortcut(open: Ref<boolean>, target: KeyTarget): void {
  const onKey = (event: Event) => {
    const key = event as KeyboardEvent;
    if ((key.metaKey || key.ctrlKey) && key.key.toLowerCase() === "k") {
      key.preventDefault();
      open.value = true;
    }
    if (key.key === "Escape" && open.value) {
      key.preventDefault();
      open.value = false;
    }
  };
  target.addEventListener("keydown", onKey);
  onScopeDispose(() => target.removeEventListener("keydown", onKey));
}

/** `source`, trimmed, once it has not changed for `ms`. */
export function useDebouncedTrim(source: Ref<string>, ms: number = SEARCH_DEBOUNCE_MS): Readonly<Ref<string>> {
  const settled = ref(source.value.trim());
  watch(source, (value, _previous, onCleanup) => {
    const handle = setTimeout(() => {
      settled.value = value.trim();
    }, ms);
    onCleanup(() => clearTimeout(handle));
  });
  return settled;
}

/**
 * The header search palette (features/workspace/search-command.tsx): the
 * workspace search of every kind in hybrid mode (the search page itself stays
 * lexical), run for the settled query only.
 */
export function useSearchPalette(options: {
  workspaceId: MaybeRefOrGetter<string>;
  keyTarget: KeyTarget;
  debounceMs?: number;
}) {
  const open = ref(false);
  const draft = ref("");
  const q = useDebouncedTrim(draft, options.debounceMs);
  useSearchShortcut(open, options.keyTarget);
  const results = useQuery(() =>
    searchQuery(toValue(options.workspaceId), q.value, "all", undefined, undefined, "hybrid"),
  );
  const items = computed(() => (results.data.value?.items ?? []) as SearchResult[]);
  return { open, draft, q, results, items };
}
