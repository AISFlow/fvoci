import { t } from "@fvoci/i18n";

export const STATUS_CATEGORIES = ["backlog", "todo", "in_progress", "done", "canceled"] as const;
export type StatusCategory = (typeof STATUS_CATEGORIES)[number];
export type StatusPatch = { name?: string; category?: StatusCategory };

export function asCategory(value: string): StatusCategory {
  return STATUS_CATEGORIES.find((category) => category === value) ?? "todo";
}

export function categoryLabel(category: StatusCategory): string {
  return t(`seed.status.${category}`);
}

export function statusPatch(
  row: { name: string; category: string },
  name: string,
  category: StatusCategory,
): StatusPatch {
  return {
    ...(name.trim() !== row.name ? { name: name.trim() } : {}),
    ...(category !== row.category ? { category } : {}),
  };
}
