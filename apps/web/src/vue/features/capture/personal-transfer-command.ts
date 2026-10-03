import type { I18nKey } from "@fvoci/i18n";
import type { QueryClient, QueryKey } from "@tanstack/query-core";
import { z } from "zod";
import type { components } from "@/generated/api";
import { ProblemError } from "@/lib/api";
import { formatDisplayId, itemPath } from "@/lib/href";
import type { CommandStorage } from "./capture-command";
export type TransferAction = components["schemas"]["PersonalTransferAction"];
export type TransferSelection = components["schemas"]["PersonalTransferSelection"];
export type TransferBody = components["schemas"]["PersonalTransferBody"];
export type TransferPreview = components["schemas"]["PersonalTransferPreview"];
export type TransferResult = components["schemas"]["PersonalTransferOutput"];
export type TransferBlocker = components["schemas"]["PersonalTransferBlocker"];
export type TransferConflict = components["schemas"]["PersonalTransferConflict"];
export type TransferDisposition = components["schemas"]["PersonalTransferDisposition"];
export type TransferOutcome = components["schemas"]["PersonalTransferOutcome"];
// Exhaustive over the generated unions: a new server value fails typecheck.
const BLOCKER_KEYS = {
  native_history: "personalTransfer.blocker.nativeHistory",
  outgoing_reference: "personalTransfer.blocker.reference",
  incoming_reference: "personalTransfer.blocker.incomingReference",
  file: "personalTransfer.blocker.file",
  hierarchy: "personalTransfer.blocker.hierarchy",
  assignee: "personalTransfer.blocker.assignee",
  dependent_graph: "personalTransfer.blocker.related",
  wip_reservation: "personalTransfer.blocker.wip",
  inventory_budget: "personalTransfer.blocker.budget",
  block_identity: "personalTransfer.blocker.body",
  body_encoding: "personalTransfer.blocker.body",
  native_state_missing: "personalTransfer.blocker.body",
} as const satisfies Record<TransferBlocker, I18nKey>;
const CONFLICT_KEYS = {
  command_changed: "personalTransfer.conflictCommand",
  preview_stale: "personalTransfer.conflict",
} as const satisfies Record<TransferConflict, I18nKey>;
export const OUTCOME_KEYS = {
  moved: "personalTransfer.outcome.moved",
  copied_new_id: "personalTransfer.outcome.copied",
  retained_private: "personalTransfer.outcome.retained",
  not_included: "personalTransfer.outcome.excluded",
} as const satisfies Record<TransferOutcome, I18nKey>;
function known<T extends object>(
  table: T,
  reason: string | undefined,
): reason is Extract<keyof T, string> {
  return reason !== undefined && Object.hasOwn(table, reason);
}
/** The personal item exactly as the host loaded it; versions come from its metadata. */
export type TransferSource = {
  workspaceId: string;
  documentId: string;
  documentVersion: number;
  taskId: string | null;
  taskVersion: number | null;
  /** The task's current personal project, for scoped cache settlement only. */
  taskProjectId: string | null;
};
export type TransferDestination = {
  workspaceId: string;
  slug: string;
  projectId: string;
  projectKey: string;
  statusId: string | null;
};
const uuid = z.string().uuid();
const commandSchema = z
  .object({
    actorId: uuid,
    sessionId: uuid,
    sourceWorkspaceId: uuid,
    /**
     * Cache settlement only; never authority. Recovery replays need it without
     * a source route. Optional so commands stored before it stay recoverable
     * with their exact request ID and body.
     */
    sourceTaskProjectId: uuid.nullable().optional(),
    destinationSlug: z.string().min(1),
    destinationProjectKey: z.string().min(1),
    body: z
      .object({
        requestId: uuid,
        confirmed: z.literal(true),
        previewDigest: z.string().regex(/^[0-9a-f]{64}$/),
        selection: z
          .object({
            action: z.enum(["copy", "move"]),
            documentId: uuid,
            expectedDocumentVersion: z.number().int().positive(),
            taskId: uuid.nullable().optional(),
            expectedTaskVersion: z.number().int().positive().nullable().optional(),
            destinationWorkspaceId: uuid,
            destinationProjectId: uuid,
            destinationStatusId: uuid.nullable().optional(),
          })
          .strict(),
      })
      .strict(),
  })
  .strict();
export type PendingTransferCommand = z.infer<typeof commandSchema>;
export const TRANSFER_COMMAND_EVENT = "fvoci:personal-transfer-command";
// The server binds a command to its authenticated session, so a retired
// session's entry can never replay. Keying by session keeps that orphan from
// blocking every later transfer in the same tab.
function key(actorId: string, sessionId: string): string {
  return `fvoci:personal-transfer:${actorId}:${sessionId}`;
}
function changed(): void {
  if (typeof window !== "undefined") window.dispatchEvent(new Event(TRANSFER_COMMAND_EVENT));
}
export function buildSelection(
  source: TransferSource,
  action: TransferAction,
  destination: Pick<TransferDestination, "workspaceId" | "projectId" | "statusId">,
): TransferSelection {
  const withTask = source.taskId !== null;
  return {
    action,
    documentId: source.documentId,
    expectedDocumentVersion: source.documentVersion,
    taskId: source.taskId,
    expectedTaskVersion: withTask ? source.taskVersion : null,
    destinationWorkspaceId: destination.workspaceId,
    destinationProjectId: destination.projectId,
    destinationStatusId: withTask ? destination.statusId : null,
  };
}
/** Recover only for the same actor AND credential; a locator never implies source access. */
export function recoverTransfer(
  storage: CommandStorage,
  actorId: string,
  sessionId: string,
): PendingTransferCommand | null {
  if (!actorId || !sessionId) return null;
  const raw = storage.getItem(key(actorId, sessionId));
  if (!raw) return null;
  try {
    const command = commandSchema.parse(JSON.parse(raw));
    return command.actorId === actorId && command.sessionId === sessionId ? command : null;
  } catch {
    return null;
  }
}
/** Persist the immutable confirmation BEFORE dispatch; an unknown outcome never rotates it. */
export function rememberTransfer(storage: CommandStorage, command: PendingTransferCommand): void {
  const parsed = commandSchema.parse(command);
  const serialized = JSON.stringify(parsed);
  const existing = storage.getItem(key(parsed.actorId, parsed.sessionId));
  if (existing !== null && existing !== serialized)
    throw new Error("pending personal transfer already exists");
  storage.setItem(key(parsed.actorId, parsed.sessionId), serialized);
  changed();
}
/** Explicit abandonment or the exact acknowledged command; a stale success cannot erase a new one. */
export function forgetTransfer(storage: CommandStorage, command: PendingTransferCommand): void {
  if (
    recoverTransfer(storage, command.actorId, command.sessionId)?.body.requestId !==
    command.body.requestId
  )
    return;
  storage.removeItem(key(command.actorId, command.sessionId));
  changed();
}
export type TransferFailure = "incomplete" | "conflict" | "unavailable" | "unknown";
/**
 * Only a definitive server answer is classified. A transport error or 5xx
 * leaves the committed outcome unknown, so the stored command is retried.
 */
export function classifyTransferFailure(error: unknown): TransferFailure {
  if (!(error instanceof ProblemError) || error.status >= 500) return "unknown";
  if (error.code === "personal_transfer_incomplete") return "incomplete";
  if (error.code === "personal_transfer_conflict") return "conflict";
  return "unavailable";
}
/**
 * Message keys for a failure, from its typed `params.code` only. The server's
 * diagnostic title is never shown. null: use the ordinary problem message.
 */
export function transferFailureKeys(error: unknown): I18nKey[] | null {
  const reason = error instanceof ProblemError ? error.reason : undefined;
  switch (classifyTransferFailure(error)) {
    case "unknown":
      return ["personalTransfer.unknown"];
    case "incomplete":
      return known(BLOCKER_KEYS, reason)
        ? [BLOCKER_KEYS[reason], "personalTransfer.incomplete"]
        : ["personalTransfer.incomplete"];
    case "conflict":
      return [known(CONFLICT_KEYS, reason) ? CONFLICT_KEYS[reason] : "personalTransfer.conflict"];
    case "unavailable":
      return null;
  }
}
export function audienceKey(
  visibility: string,
): "personalTransfer.audienceWorkspace" | "personalTransfer.audiencePrivate" {
  // Unknown values are described narrowly: never imply a wider audience.
  return visibility === "workspace"
    ? "personalTransfer.audienceWorkspace"
    : "personalTransfer.audiencePrivate";
}
/** What a host shows but has not durably saved; read fresh on every check. */
export type HostDraftSnapshot = {
  /** Committed metadata the preview digest describes; absent while loading. */
  committed: { title: string; icon: string | null; status: string } | undefined;
  title: string;
  icon: string;
  status: string;
  /** A metadata save in flight. */
  saving: boolean;
  /** Unapplied Markdown source: editor's live state and the last one received. */
  sourceDrafts: readonly ({ dirty: boolean; composing: boolean } | null | undefined)[];
};
function hostDraftsClean(snapshot: HostDraftSnapshot): boolean {
  const committed = snapshot.committed;
  return (
    !!committed &&
    !snapshot.saving &&
    snapshot.sourceDrafts.every((draft) => !draft?.dirty && !draft?.composing) &&
    // Same normalization as the host's own title/icon saves.
    snapshot.title.trim() === committed.title &&
    (snapshot.icon.trim() || null) === committed.icon &&
    snapshot.status === committed.status
  );
}
/**
 * A transfer publishes committed content only. Refuse while any host draft
 * (unapplied or composing Markdown, a dirty, pending or failed header field)
 * is unsaved, before and after the host's real durable-save barrier. Drafts
 * are never applied, saved or discarded here.
 */
export async function hostTransferPrepare(
  snapshot: () => HostDraftSnapshot,
  persist: () => Promise<boolean>,
): Promise<boolean> {
  if (!hostDraftsClean(snapshot())) return false;
  const saved = await persist();
  return saved && hostDraftsClean(snapshot());
}
/** The task host's identity and fences; read fresh on every check. */
export type TaskHostSnapshot = {
  workspaceId: string;
  taskId: string;
  /** Auth actor and auth session (never the collaboration room's session). */
  actorId: string;
  sessionId: string;
  /** Bumped synchronously on any identity, session or form-epoch change (ABA). */
  generation: number;
  /** Host-owned synchronous pending flags (metadata, archive, trash, clone, delete). */
  busy: boolean;
  /** The mounted form's getter; null while it is not mounted. */
  draft: Readonly<{
    workspaceId: string;
    taskId: string;
    actorId: string;
    hasUnsavedMetadata: boolean;
    pending: boolean | undefined;
  }> | null;
  /** Body room generation, and edits it has not acknowledged yet. */
  bodyGeneration: number | null;
  bodyPending: boolean;
};
function taskHostClean(now: TaskHostSnapshot, opened: TaskHostSnapshot): boolean {
  const draft = now.draft;
  return (
    !!draft &&
    !!now.actorId &&
    !!now.sessionId &&
    !now.busy &&
    now.generation === opened.generation &&
    now.workspaceId === opened.workspaceId &&
    now.taskId === opened.taskId &&
    now.actorId === opened.actorId &&
    now.sessionId === opened.sessionId &&
    now.bodyGeneration === opened.bodyGeneration &&
    draft.workspaceId === now.workspaceId &&
    draft.taskId === now.taskId &&
    draft.actorId === now.actorId &&
    !draft.hasUnsavedMetadata &&
    !draft.pending
  );
}
/**
 * The task host's transfer barrier: refuse while the mounted form holds an
 * unsaved or pending metadata draft or the host is busy, wait for the body's
 * durable save, then refuse unless the same host (workspace, task, auth actor
 * and session, generation) is still clean and the body has nothing
 * unacknowledged. Drafts are never applied, saved or discarded here.
 */
export async function taskTransferPrepare(
  snapshot: () => TaskHostSnapshot,
  persistBody: () => Promise<boolean>,
): Promise<boolean> {
  const opened = snapshot();
  if (!taskHostClean(opened, opened)) return false;
  const saved = await persistBody();
  const now = snapshot();
  return saved && !now.bodyPending && taskHostClean(now, opened);
}
/**
 * The origin document a task transfers with: only a task in the actor's own
 * personal workspace with exactly one origin, which is this task. Elsewhere
 * the task page mounts no transfer at all.
 */
export function taskTransferDocument(
  workspaces: readonly { id: string; kind: string }[] | undefined,
  workspaceId: string,
  taskId: string,
  origins: { count: number; items: readonly { documentId: string; taskId: string }[] } | undefined,
): string | null {
  const personal = (workspaces ?? []).some(
    (workspace) => workspace.id === workspaceId && workspace.kind === "personal",
  );
  if (!personal || origins?.count !== 1) return null;
  const origin = origins.items[0];
  return origin?.taskId === taskId ? origin.documentId : null;
}
/**
 * Settle one exact retained query the way task-cache's keeping-load-more
 * invalidation does (an in-flight "load more" finishes, then every loaded
 * page refetches again so a page fetched before the transfer cannot stay),
 * but let a real refetch failure reject: the caller then keeps its stored
 * command instead of declaring a stale cache settled.
 */
export async function settleExactKeepingLoadMore(
  client: QueryClient,
  queryKey: QueryKey,
): Promise<void> {
  const filters = { queryKey, exact: true };
  const inFlight = client.isFetching(filters) > 0;
  await client.invalidateQueries(filters, { cancelRefetch: false, throwOnError: true });
  if (inFlight)
    await client.invalidateQueries(filters, { cancelRefetch: false, throwOnError: true });
}
/** A retained query as the cache holds it; only read, never mutated. */
export type RetainedQuery = { queryKey: readonly unknown[]; data: unknown };
function holdsTask(value: unknown, task: string, depth = 0): boolean {
  if (depth > 8 || typeof value !== "object" || value === null) return false;
  if (Array.isArray(value)) return value.some((item) => holdsTask(item, task, depth + 1));
  const row = value as Record<string, unknown>;
  if (row.id === task || row.taskId === task) return true;
  return Object.values(row).some((item) => holdsTask(item, task, depth + 1));
}
function taskProjectIn(value: unknown, task: string, depth = 0): string | null {
  if (depth > 8 || typeof value !== "object" || value === null) return null;
  if (Array.isArray(value)) {
    for (const item of value) {
      const found = taskProjectIn(item, task, depth + 1);
      if (found) return found;
    }
    return null;
  }
  const row = value as Record<string, unknown>;
  if (row.id === task && typeof row.projectId === "string") return row.projectId;
  for (const item of Object.values(row)) {
    const found = taskProjectIn(item, task, depth + 1);
    if (found) return found;
  }
  return null;
}
const PROJECT_SCOPED = new Set(["tasks", "task-layout", "project-collection"]);
/**
 * The moved task's source project, only from retained data that positively
 * contains that task. Two different answers are ambiguous: never guess.
 */
export function retainedTaskProject(
  queries: readonly RetainedQuery[],
  workspaceId: string,
  taskId: string,
): string | null {
  const found = new Set<string>();
  for (const { queryKey: key, data } of queries) {
    if (key[1] !== workspaceId) continue;
    if (typeof key[0] === "string" && PROJECT_SCOPED.has(key[0]) && typeof key[2] === "string") {
      if (holdsTask(data, taskId)) found.add(key[2]);
    } else if (key[0] === "workspace-tasks") {
      const project = taskProjectIn(data, taskId);
      if (project) found.add(project);
    }
  }
  return found.size === 1 ? ([...found][0] ?? null) : null;
}
/**
 * Exact retained source keys that do not need a project: the task's own
 * families, MyTasks/home preview, project counts, task search and the
 * document/task origin and backlink families. Nothing outside the workspace.
 */
export function unscopedSourceKeys(
  queries: readonly RetainedQuery[],
  workspaceId: string,
  taskId: string,
  documentId: string,
): (readonly unknown[])[] {
  return queries
    .filter(({ queryKey: key, data }) => {
      if (key[0] === "backlinks")
        return key[2] === workspaceId && (key[3] === taskId || holdsTask(data, taskId));
      if (key[1] !== workspaceId) return false;
      switch (key[0]) {
        case "task":
        case "task-activity":
        case "task-time-entries":
          return key[2] === taskId;
        case "collection-item":
          return key[2] === "task" && key[3] === taskId;
        case "task-origins":
          return key[2] === taskId || key[2] === documentId;
        case "workspace-tasks":
        case "projects":
        case "collection":
          return true;
        case "search":
          return key[3] === "all" || key[3] === "task";
        default:
          return false;
      }
    })
    .map(({ queryKey }) => queryKey);
}
/** Destination routes come from the committed numbers, never the source route. */
export function resultPaths(
  command: Pick<PendingTransferCommand, "destinationSlug" | "destinationProjectKey">,
  result: TransferResult,
): { document: string; task: string | null } {
  const at = (number: number) =>
    itemPath(command.destinationSlug, formatDisplayId(command.destinationProjectKey, number));
  return {
    document: at(result.documentNumber),
    task:
      result.taskNumber === null || result.taskNumber === undefined ? null : at(result.taskNumber),
  };
}
