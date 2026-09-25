import ko from "./locales/ko.json";

const CATALOG: Record<string, string> = ko;

export type I18nKey = keyof typeof ko;

export function isI18nKey(key: string): key is I18nKey {
  return Object.hasOwn(CATALOG, key);
}

export function t(key: I18nKey, opts?: Record<string, unknown>): string {
  const raw = CATALOG[key];
  if (raw === undefined) {
    throw new Error(`missing i18n key: ${key}`);
  }
  if (!opts) return raw;
  return raw.replace(/\{\{(\w+)\}\}/g, (match, name: string) => {
    const value = opts[name];
    return value === undefined ? match : String(value);
  });
}

const PROBLEM_TITLES: Record<string, I18nKey> = {
  authentication_required: "authentication required",
  invalid_email_or_password: "invalid email or password",
  invalid_input: "invalid input",
  instance_setup_already_completed: "instance setup already completed",
  slug_taken: "slug taken",
  not_found: "not found",
  insufficient_permissions: "insufficient permissions",
  personal_workspace_is_immutable: "personal workspace is immutable",
  origin_mismatch: "origin mismatch",
  rate_limit_exceeded: "Rate limit exceeded",
  conflict: "conflict",
  project_archived: "project.archivedReadOnly",
  restore_rejected: "restore rejected",
  collab_timeout_retry: "collab timeout — retry",
  cannot_invite_a_role_above_your_own: "cannot invite a role above your own",
  cannot_manage_a_role_above_your_own: "cannot manage a workspace role above your own",
  invitation_not_found_or_expired: "invitation not found or expired",
  expired: "expired",
  already_accepted: "already_accepted",
  cannot_accept_invitation: "cannot accept invitation",
  consent_required: "consent_required",
  "limit.seats": "seat limit reached",
  "limit.guests": "guest limit reached",
  internal_error: "error.http.fallback",
  document_version_mismatch: "document version mismatch (optimistic lock)",
  wip_limit_exceeded: "wip limit exceeded",
  task_hierarchy_violation: "task hierarchy violation",
  invalid_recurrence_preset: "invalid recurrence preset",
  task_archived: "task is archived — read-only",
  search_unavailable: "search unavailable",
  status_not_in_project_workflow: "status not in project workflow",
  assignee_is_not_a_member: "assignee is not a member",
};

export function tProblemTitle(
  code: string,
  params?: Readonly<Record<string, string | number>>,
): string {
  const key = PROBLEM_TITLES[code];
  if (key && isI18nKey(key)) {
    return t(key, params);
  }
  return t("error.http.fallback");
}

export function formatPersonName(
  name: { givenName: string; familyName?: string | null },
  locale = "ko",
): string {
  const family = name.familyName?.trim() ?? "";
  const given = name.givenName.trim();
  if (family === "") return given;
  return locale === "ko" ? `${family}${given}` : `${given} ${family}`;
}
