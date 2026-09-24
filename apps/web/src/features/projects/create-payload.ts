import { canonicalizeProjectKey, projectKeyIssue } from "@/lib/href";

export const PROJECT_NAME_MAX = 200;
export const PROJECT_DESCRIPTION_MAX = 2000;
export const PROJECT_ICON_MAX = 50;

export type ProjectCreateFormValues = {
  key: string;
  name: string;
  visibility: "workspace" | "private";
  description?: string;
  icon?: string;
};

export type ProjectCreateBody = {
  key: string;
  name: string;
  visibility: "workspace" | "private";
  description: string | null;
  icon: string | null;
};

export type ProjectCreateIssue =
  | { field: "key"; code: "reserved" | "pattern" | "too_small" }
  | { field: "name" | "description" | "icon"; code: "too_small" | "too_big" };

function blankToNull(value: string | undefined): string | null {
  const trimmed = value?.trim() ?? "";
  return trimmed === "" ? null : trimmed;
}

/** Source `projectCreateInput` strict: NFKC key, trim name, blank description/icon → null, no extra keys. */
export function projectCreatePayload(
  values: ProjectCreateFormValues,
): { ok: true; body: ProjectCreateBody } | { ok: false; issue: ProjectCreateIssue } {
  const key = canonicalizeProjectKey(values.key);
  if (key === "") return { ok: false, issue: { field: "key", code: "too_small" } };
  const keyIssue = projectKeyIssue(key);
  if (keyIssue === "reserved" || keyIssue === "pattern") {
    return { ok: false, issue: { field: "key", code: keyIssue } };
  }
  const name = values.name.trim();
  if (name.length < 1) return { ok: false, issue: { field: "name", code: "too_small" } };
  if (name.length > PROJECT_NAME_MAX) return { ok: false, issue: { field: "name", code: "too_big" } };
  const description = blankToNull(values.description);
  if (description !== null && description.length > PROJECT_DESCRIPTION_MAX) {
    return { ok: false, issue: { field: "description", code: "too_big" } };
  }
  const icon = blankToNull(values.icon);
  if (icon !== null && icon.length > PROJECT_ICON_MAX) {
    return { ok: false, issue: { field: "icon", code: "too_big" } };
  }
  return {
    ok: true,
    body: {
      key,
      name,
      visibility: values.visibility,
      description,
      icon,
    },
  };
}
