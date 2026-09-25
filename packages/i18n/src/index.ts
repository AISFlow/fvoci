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
  password_invalid: "password_invalid",
  magic_invalid: "magic_invalid",
  confirm_invalid: "confirm_invalid",
  owner_transfer_required: "owner_transfer_required",
  last_instance_admin: "last_instance_admin",
  document_version_mismatch: "document version mismatch (optimistic lock)",
  wip_limit_exceeded: "wip limit exceeded",
  task_hierarchy_violation: "task hierarchy violation",
  invalid_recurrence_preset: "invalid recurrence preset",
  task_archived: "task is archived — read-only",
  search_unavailable: "search unavailable",
  status_not_in_project_workflow: "status not in project workflow",
  assignee_is_not_a_member: "assignee is not a member",
  dependency_cycle: "dependency cycle",
  dependency_contradiction: "dependency contradiction",
  task_cannot_block_itself: "task cannot block itself",
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

function payloadString(value: unknown): string | null {
  return typeof value === "string" && value.length > 0 ? value : null;
}

function roParticle(word: string): string {
  const code = [...word].at(-1)?.codePointAt(0) ?? 0;
  if (code < 0xac00 || code > 0xd7a3) return t("particle.euroParen");
  return (code - 0xac00) % 28 === 0 ? t("particle.ro") : t("particle.euro");
}

const ROLE_LABEL = {
  lead: "projectRole.lead",
  member: "projectRole.member",
  viewer: "projectRole.viewer",
} as const satisfies Record<string, I18nKey>;

function roleLabel(payload: Record<string, unknown>): string | null {
  const raw = payloadString(payload.role);
  if (raw === null || !(raw in ROLE_LABEL)) return null;
  return t(ROLE_LABEL[raw as keyof typeof ROLE_LABEL]);
}

function taskRef(payload: Record<string, unknown>): string | null {
  const title = payloadString(payload.title);
  const number = typeof payload.number === "number" ? `#${payload.number}` : null;
  if (title && number) return `${number} 「${title}」`;
  if (title) return `「${title}」`;
  return number;
}

export function notificationMessage(item: {
  verb: string;
  payload: Record<string, unknown> | null | undefined;
}): string {
  const payload = item.payload ?? {};
  const ref = taskRef(payload);
  switch (item.verb) {
    case "task.created":
      return ref ? t("notif.task.created.ref", { ref }) : t("notif.task.created");
    case "task.updated": {
      const fromName = payloadString(payload.fromName);
      const toName = payloadString(payload.toName);
      if (fromName && toName) {
        return ref
          ? t("notif.task.status", { ref, from: fromName, to: toName })
          : t("notif.task.statusPlain", { from: fromName, to: toName });
      }
      return ref ? t("notif.task.assignee.ref", { ref }) : t("notif.task.assignee");
    }
    case "task.deleted":
      return typeof payload.number === "number"
        ? t("notif.task.deleted.number", { number: payload.number })
        : t("notif.task.deleted");
    case "project_member.added":
    case "project_member.role_changed": {
      const name = payloadString(payload.projectName);
      const role = roleLabel(payload);
      const target = name ? t("notif.project.named", { name }) : t("nav.projects");
      const added = item.verb === "project_member.added";
      if (role === null) {
        return added
          ? t("notif.project.added", { target })
          : t("notif.project.role.changed", { target });
      }
      const opts = { target, role, particle: roParticle(role) };
      return added
        ? t("notif.project.added.role", opts)
        : t("notif.project.role.changed.role", opts);
    }
    case "invitation.accepted":
      return t("notif.invite.accepted");
    case "comment.created": {
      if (payloadString(payload.parentId)) return t("notif.comment.reply");
      if (payloadString(payload.taskId)) return t("notif.comment.task");
      if (payloadString(payload.documentId)) return t("notif.comment.document");
      return t("notif.comment.created");
    }
    case "comment.resolved": {
      if (payloadString(payload.taskId)) return t("notif.comment.resolved.task");
      if (payloadString(payload.documentId)) return t("notif.comment.resolved.document");
      return t("notif.comment.resolved");
    }
    default:
      return t("notif.generic");
  }
}
