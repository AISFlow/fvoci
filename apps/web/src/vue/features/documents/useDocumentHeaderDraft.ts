import { ref, watch } from "vue";

type Header = { title: string; icon: string | null; status: string };

/** Own only editable header fields; the caller owns query and permission lifetimes. */
export function useDocumentHeaderDraft(metadata: () => Header | undefined) {
  const title = ref("");
  const icon = ref("");
  const status = ref("draft");
  let confirmed: { title: string; icon: string; status: string } | undefined;

  function synchronize(data: Header | undefined, reset = false): void {
    const next = data && { title: data.title, icon: data.icon ?? "", status: data.status };
    if (reset || !confirmed || title.value === confirmed.title) title.value = next?.title ?? "";
    if (reset || !confirmed || icon.value === confirmed.icon) icon.value = next?.icon ?? "";
    if (reset || !confirmed || status.value === confirmed.status)
      status.value = next?.status ?? "draft";
    confirmed = next;
  }

  watch(
    metadata,
    (data) => {
      synchronize(data);
    },
    { immediate: true },
  );
  return {
    title,
    icon,
    status,
    reset: () => {
      synchronize(metadata(), true);
    },
  };
}
