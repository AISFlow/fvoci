import type { I18nKey } from "@fvoci/i18n";
import {
  PROJECT_DESCRIPTION_MAX,
  PROJECT_ICON_MAX,
  PROJECT_NAME_MAX,
  type ProjectCreateBody,
  type ProjectCreateFormValues,
  projectCreatePayload,
} from "@/features/projects/create-payload";

export type ProjectFormField = "key" | "name" | "description" | "icon";

export type ProjectFormResult =
  { ok: true; body: ProjectCreateBody } | { ok: false; field: ProjectFormField; message: I18nKey };

function issue(field: ProjectFormField, message: I18nKey): ProjectFormResult {
  return { ok: false, field, message };
}

/**
 * The project create and clone forms' checks (the React forms ran a zod schema
 * over the fields as typed, then the source's strict payload rules): the body
 * to send, or the message for the first field that fails, in the order key,
 * name, description, icon.
 */
export function projectFormPayload(values: ProjectCreateFormValues): ProjectFormResult {
  if (values.key.length < 1) return issue("key", "form.too_small");
  const name = values.name.trim();
  if (name.length < 1) return issue("name", "form.too_small");
  if (name.length > PROJECT_NAME_MAX) return issue("name", "form.too_big");
  if ((values.description ?? "").length > PROJECT_DESCRIPTION_MAX)
    return issue("description", "form.too_big");
  if ((values.icon ?? "").length > PROJECT_ICON_MAX) return issue("icon", "form.too_big");

  const parsed = projectCreatePayload(values);
  if (parsed.ok) return parsed;
  const { field, code } = parsed.issue;
  if (field === "key" && code === "reserved") return issue("key", "project.key.reserved");
  if (field === "key" && code === "pattern") return issue("key", "form.pattern.key");
  return issue(field, code === "too_big" ? "form.too_big" : "form.too_small");
}

const COPY_SUFFIX = " (복사)";

/** The clone form's default name: the source's name marked as a copy, within the name limit. */
export function cloneDefaultName(sourceName: string): string {
  return sourceName.length + COPY_SUFFIX.length > PROJECT_NAME_MAX
    ? sourceName.slice(0, PROJECT_NAME_MAX - COPY_SUFFIX.length) + COPY_SUFFIX
    : `${sourceName}${COPY_SUFFIX}`;
}
