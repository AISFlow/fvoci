import type { EntityResolver, EntitySnapshot } from "../../../../../packages/editor/src/entities";
import type { MentionHit } from "../../../../../packages/editor/src/editor-extensions";
import { formatPersonName, t } from "@fvoci/i18n";
// Reuse the editor's neutral identifier helpers without loading its Vue host
// (the public Vue entry includes SFCs). No new identifier grammar here.
import { formatDisplayId, parseDisplayId } from "../../../../../packages/editor/src/display-id";
import { uuid } from "../../../../../packages/editor/src/uuid";
import type { components } from "@/generated/api";
import { api, ensureOk, ProblemError } from "@/lib/api";

type Schema = components["schemas"];
/** Only the existing authorized endpoints used by editor metadata. */
export interface EditorEntityTransport {
  members(workspaceId: string, signal: AbortSignal): Promise<Schema["MembersResponse"]>;
  groups(workspaceId: string, signal: AbortSignal): Promise<Schema["GroupListResponse"]>;
  lookup(
    workspaceId: string,
    displayId: string,
    signal: AbortSignal,
  ): Promise<Schema["LookupListResponse"]>;
  search(
    workspaceId: string,
    q: string,
    kind: "task" | "document",
    signal: AbortSignal,
  ): Promise<Schema["SearchListResponse"]>;
  projects(workspaceId: string, signal: AbortSignal): Promise<Schema["ProjectListResponse"]>;
  task(workspaceId: string, id: string, signal: AbortSignal): Promise<Schema["TaskOutput"]>;
  documentUuid(id: string, signal: AbortSignal): Promise<Schema["DocumentMetaResponse"]>;
  document(
    workspaceId: string,
    id: string,
    projectId: string | null,
    signal: AbortSignal,
  ): Promise<Schema["DocumentMetaResponse"]>;
  workflow(
    workspaceId: string,
    projectId: string,
    signal: AbortSignal,
  ): Promise<Schema["WorkflowOutput"]>;
}

export const editorEntityTransport: EditorEntityTransport = {
  members: async (workspace_id, signal) =>
    ensureOk(
      await api.GET("/api/v1/workspaces/{workspace_id}/members", {
        params: { path: { workspace_id } },
        signal,
      }),
    ),
  groups: async (workspace_id, signal) =>
    ensureOk(
      await api.GET("/api/v1/workspaces/{workspace_id}/groups", {
        params: { path: { workspace_id } },
        signal,
      }),
    ),
  lookup: async (workspace_id, display_id, signal) =>
    ensureOk(
      await api.GET("/api/v1/workspaces/{workspace_id}/lookup/{display_id}", {
        params: { path: { workspace_id, display_id } },
        signal,
      }),
    ),
  search: async (workspace_id, q, type, signal) =>
    ensureOk(
      await api.GET("/api/v1/workspaces/{workspace_id}/search", {
        params: { path: { workspace_id }, query: { q, type, mode: "lexical", limit: 50 } },
        signal,
      }),
    ),
  projects: async (workspace_id, signal) =>
    ensureOk(
      await api.GET("/api/v1/workspaces/{workspace_id}/projects", {
        params: { path: { workspace_id } },
        signal,
      }),
    ),
  task: async (workspace_id, task_id, signal) =>
    ensureOk(
      await api.GET("/api/v1/workspaces/{workspace_id}/tasks/{task_id}", {
        params: { path: { workspace_id, task_id } },
        signal,
      }),
    ),
  documentUuid: async (document_id, signal) =>
    ensureOk(
      await api.GET("/api/v1/documents/{document_id}", {
        params: { path: { document_id } },
        signal,
      }),
    ),
  document: async (workspace_id, document_id, project_id, signal) =>
    project_id === null
      ? ensureOk(
          await api.GET("/api/v1/workspaces/{workspace_id}/documents/{document_id}", {
            params: { path: { workspace_id, document_id } },
            signal,
          }),
        )
      : ensureOk(
          await api.GET(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}",
            {
              params: { path: { workspace_id, project_id, document_id } },
              signal,
            },
          ),
        ),
  workflow: async (workspace_id, project_id, signal) =>
    ensureOk(
      await api.GET("/api/v1/workspaces/{workspace_id}/projects/{project_id}/workflow", {
        params: { path: { workspace_id, project_id } },
        signal,
      }),
    ),
};

function named(value: string | null | undefined): string | null {
  return value?.trim() || null;
}

function snapshot(
  label: string | null | undefined,
  icon = "",
  status?: string,
): EntitySnapshot | null {
  const name = named(label);
  const normalizedStatus = named(status);
  return name
    ? {
        label: name,
        icon: named(icon) ?? "",
        ...(normalizedStatus ? { status: normalizedStatus } : {}),
      }
    : null;
}

function entityHit(
  kind: string,
  id: string,
  label: string | null,
  title: string,
): MentionHit | null {
  if ((kind !== "task" && kind !== "document") || !id || label === null || !named(label))
    return null;
  return { entity: kind, id, label, title };
}

export function createWorkspaceEditorEntities(
  workspaceId: string,
  transport: EditorEntityTransport = editorEntityTransport,
) {
  let alive = Boolean(workspaceId);
  const isAlive = () => alive;
  let mentionEpoch = 0;
  let mention: {
    query: string;
    controller: AbortController;
    promise: Promise<MentionHit[]>;
  } | null = null;
  const controllers = new Set<AbortController>();
  const requests = new Map<string, Promise<unknown>>();
  const entities = new Map<string, Promise<EntitySnapshot | null>>();

  function dispose(): void {
    alive = false;
    mentionEpoch++;
    for (const controller of controllers) controller.abort();
    controllers.clear();
    requests.clear();
    entities.clear();
    mention = null;
  }

  async function authorized<T>(load: () => Promise<T>): Promise<T> {
    try {
      return await load();
    } catch (error) {
      // A denied members API is expected for guests; expired authentication
      // instead ends the whole scope, including other pending enrichment.
      if (error instanceof ProblemError && error.status === 401) dispose();
      throw error;
    }
  }

  async function optional<T>(load: () => Promise<T>, fallback: T): Promise<T> {
    try {
      return await authorized(load);
    } catch {
      return fallback;
    }
  }

  // Only inflight results are shared. Every later lookup revalidates access.
  function request<T>(key: string, load: (signal: AbortSignal) => Promise<T>): Promise<T> {
    const existing = requests.get(key);
    if (existing) return existing as Promise<T>;
    const controller = new AbortController();
    controllers.add(controller);
    const promise = authorized(() => load(controller.signal)).finally(() => {
      controllers.delete(controller);
      if (requests.get(key) === promise) requests.delete(key);
    });
    requests.set(key, promise);
    return promise;
  }

  function lookup(id: string) {
    const parsed = parseDisplayId(id);
    if (!parsed) return null;
    const displayId = formatDisplayId(parsed.prefix, parsed.n);
    return request(`lookup:${displayId}`, (signal) =>
      transport.lookup(workspaceId, displayId, signal),
    );
  }

  function mentionItems(raw: string): Promise<MentionHit[]> {
    if (!alive) return Promise.resolve([]);
    const query = raw.trim();
    if (mention?.query === query) return mention.promise;
    mention?.controller.abort();
    const epoch = ++mentionEpoch;
    const controller = new AbortController();
    controllers.add(controller);
    const signal = controller.signal;
    const parsed = parseDisplayId(query);
    const hits = parsed
      ? optional(
          () => transport.lookup(workspaceId, formatDisplayId(parsed.prefix, parsed.n), signal),
          { items: [] },
        ).then((page) =>
          page.items.map((item) => entityHit(item.kind, item.id, item.displayId, item.title)),
        )
      : query
        ? Promise.all([
            optional(() => transport.search(workspaceId, query, "task", signal), {
              items: [],
              nextCursor: null,
            }),
            optional(() => transport.search(workspaceId, query, "document", signal), {
              items: [],
              nextCursor: null,
            }),
          ]).then(([tasks, documents]) =>
            [...tasks.items, ...documents.items].map((item) =>
              entityHit(item.type, item.id, item.displayId, item.title),
            ),
          )
        : Promise.resolve([]);
    const promise = Promise.all([
      hits,
      optional(() => transport.members(workspaceId, signal), { items: [] }),
      optional(() => transport.groups(workspaceId, signal), { items: [] }),
    ])
      .then(([hits, members, groups]) => {
        if (!alive || signal.aborted || epoch !== mentionEpoch) return [];
        const people: MentionHit[] = [
          ...members.items
            .filter((m) => m.userId)
            .map((m) => ({
              entity: "user" as const,
              id: m.userId,
              label: formatPersonName(m),
              title: formatPersonName(m),
            })),
          ...groups.items
            .filter((g) => g.id)
            .map((g) => ({ entity: "group" as const, id: g.id, label: g.name, title: g.name })),
        ].filter((person) => person.label.toLowerCase().includes(query.toLowerCase()));
        return [...hits.filter((hit): hit is MentionHit => hit !== null), ...people];
      })
      .finally(() => {
        controllers.delete(controller);
        if (mention?.promise === promise) mention = null;
      });
    mention = { query, controller, promise };
    return promise;
  }

  const entityResolver: EntityResolver = (entity, raw) => {
    if (!alive || !raw.trim()) return Promise.resolve(null);
    const id = raw.trim();
    const key = `${entity}:${id}`;
    const existing = entities.get(key);
    if (existing) return existing;
    const promise = resolve()
      .then(
        (result) => (alive ? result : null),
        () => null,
      )
      .finally(() => {
        if (entities.get(key) === promise) entities.delete(key);
      });
    entities.set(key, promise);
    return promise;

    async function resolve(): Promise<EntitySnapshot | null> {
      const isUuid = uuid.safeParse(id).success;
      switch (entity) {
        case "user": {
          if (!isUuid) return null;
          const page = await request("members", (signal) => transport.members(workspaceId, signal));
          const member = page.items.find((item) => item.userId === id);
          return member ? snapshot(formatPersonName(member)) : null;
        }
        case "group": {
          if (!isUuid) return null;
          const page = await request("groups", (signal) => transport.groups(workspaceId, signal));
          return snapshot(page.items.find((item) => item.id === id)?.name);
        }
        case "project": {
          const page = await request("projects", (signal) =>
            transport.projects(workspaceId, signal),
          );
          const project = page.items.find((item) =>
            isUuid ? item.id === id : item.key.toUpperCase() === id.toUpperCase(),
          );
          return project
            ? snapshot(
                project.name,
                project.icon ?? "",
                project.status === "archived" ? t("project.archived.badge") : undefined,
              )
            : null;
        }
        case "document": {
          let doc: Schema["DocumentMetaResponse"];
          if (isUuid) {
            doc = await request(`document:${id}`, (signal) => transport.documentUuid(id, signal));
          } else {
            const page = lookup(id);
            const hit = page ? (await page).items.find((item) => item.kind === "document") : null;
            if (!hit || !alive) return null;
            doc = await request(`document:${String(hit.projectId)}:${hit.id}`, (signal) =>
              transport.document(workspaceId, hit.id, hit.projectId, signal),
            );
          }
          return doc.workspaceId === workspaceId ? snapshot(doc.title, doc.icon ?? "") : null;
        }
        case "task": {
          const page = isUuid ? null : lookup(id);
          const taskId = isUuid
            ? id
            : page
              ? (await page).items.find((item) => item.kind === "task")?.id
              : null;
          if (!taskId || !alive) return null;
          const task = await request(`task:${taskId}`, (signal) =>
            transport.task(workspaceId, taskId, signal),
          );
          if (!isAlive() || task.workspaceId !== workspaceId) return null;
          const wf = await optional(
            () =>
              request(`workflow:${task.projectId}`, (signal) =>
                transport.workflow(workspaceId, task.projectId, signal),
              ),
            null,
          );
          return snapshot(
            task.title,
            "",
            wf?.statuses.find((status) => status.id === task.statusId)?.name,
          );
        }
      }
    }
  };

  return { mentionItems, entityResolver, dispose };
}
