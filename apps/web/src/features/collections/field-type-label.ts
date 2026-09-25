import { t } from "@fvoci/i18n";
import { isFieldType, type FieldType } from "@/lib/collection-values";

const LABEL_KEYS = {
  text: "collection.type.text",
  paragraph: "collection.type.paragraph",
  number: "collection.type.number",
  date: "collection.type.date",
  datetime: "collection.type.datetime",
  checkbox: "collection.type.checkbox",
  select: "collection.type.select",
  multi_select: "collection.type.multi_select",
  checkboxes: "collection.type.checkboxes",
  user: "collection.type.user",
  user_multi: "collection.type.user_multi",
  labels: "collection.type.labels",
} as const satisfies Record<FieldType, Parameters<typeof t>[0]>;

export function fieldTypeLabel(type: string): string {
  return isFieldType(type) ? t(LABEL_KEYS[type]) : type;
}
