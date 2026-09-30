import { onScopeDispose, ref, watch } from "vue";
import { loadErrorMessage } from "@/lib/api";

/** Route failures belong to the page and navigation attempt that started them. */
export function useNavigationError(currentPage: () => string) {
  const error = ref<string | null>(null);
  let version = 0;
  let live = true;
  watch(
    currentPage,
    () => {
      version++;
      error.value = null;
    },
    { flush: "sync" },
  );
  onScopeDispose(() => {
    live = false;
    version++;
  });

  function run(operation: () => Promise<unknown>): void {
    const page = currentPage();
    const attempt = ++version;
    error.value = null;
    operation().catch((failure: unknown) => {
      if (live && attempt === version && currentPage() === page)
        error.value = loadErrorMessage(failure);
    });
  }
  return { error, run };
}
