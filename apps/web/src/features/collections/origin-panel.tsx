import { t } from "@fvoci/i18n";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { api, ensureOk, ProblemError } from "@/lib/api";
import { itemPath } from "@/lib/href";
import { originCreateSurface } from "./origin-create-surface";

type OriginPanelProps = {
  workspaceId: string;
  slug: string;
  documentId?: string;
  taskId?: string;
  hideWhenEmpty?: boolean;
};

function errorText(error: unknown, fallback: string): string {
  return error instanceof ProblemError ? error.title : fallback;
}

function preventImeSubmit(event: React.KeyboardEvent<HTMLFormElement>) {
  if (event.key === "Enter" && event.nativeEvent.isComposing) {
    event.preventDefault();
  }
}

/** Current permission, rather than permission when the link was created, controls each page. */
export function OriginPanel({ workspaceId, slug, documentId, taskId, hideWhenEmpty }: OriginPanelProps) {
  const queryClient = useQueryClient();
  const [after, setAfter] = useState<string | null>(null);
  const [projectId, setProjectId] = useState("");
  const [title, setTitle] = useState("");
  const [requestId, setRequestId] = useState(() => crypto.randomUUID());
  const [projectName, setProjectName] = useState("");
  const [projectKey, setProjectKey] = useState("");

  const origins = useQuery({
    queryKey: ["task-origins", workspaceId, documentId ?? taskId, after],
    enabled: Boolean(workspaceId && (documentId || taskId)),
    retry: false,
    queryFn: async () => documentId
      ? ensureOk(await api.GET("/api/v1/workspaces/{workspace_id}/documents/{document_id}/task-origins", {
          params: { path: { workspace_id: workspaceId, document_id: documentId }, query: { after: after ?? undefined, limit: 50 } },
        }))
      : ensureOk(await api.GET("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/origin", {
          params: { path: { workspace_id: workspaceId, task_id: taskId! }, query: { after: after ?? undefined, limit: 50 } },
        })),
  });
  const projects = useQuery({
    queryKey: ["task-projects", workspaceId, documentId],
    enabled: Boolean(documentId),
    retry: false,
    queryFn: async () => ensureOk(await api.GET("/api/v1/workspaces/{workspace_id}/documents/{document_id}/task-projects", {
      params: { path: { workspace_id: workspaceId, document_id: documentId! } },
    })),
  });
  useEffect(() => {
    if (!projects.data) return;
    if (!projects.data.items.some((item) => item.id === projectId)) {
      setProjectId(projects.data.suggestedId ?? "");
    }
  }, [projects.data, projectId]);

  const createTask = useMutation({
    mutationFn: async () => ensureOk(await api.POST("/api/v1/workspaces/{workspace_id}/documents/{document_id}/tasks", {
      params: { path: { workspace_id: workspaceId, document_id: documentId! } },
      body: { projectId, requestId, task: { title: title.trim() } },
    })),
    onSuccess: async () => {
      setAfter(null);
      setTitle("");
      setRequestId(crypto.randomUUID());
      await queryClient.invalidateQueries({ queryKey: ["task-origins", workspaceId, documentId] });
      await queryClient.invalidateQueries({ queryKey: ["tasks", workspaceId] });
    },
  });
  const createProject = useMutation({
    mutationFn: async () => ensureOk(await api.POST("/api/v1/workspaces/{workspace_id}/projects", {
      params: { path: { workspace_id: workspaceId } },
      body: { key: projectKey.trim().toUpperCase(), name: projectName.trim(), visibility: "workspace" },
    })),
    onSuccess: async (project) => {
      setProjectId(project.id);
      setRequestId(crypto.randomUUID());
      setProjectName("");
      setProjectKey("");
      await queryClient.invalidateQueries({ queryKey: ["task-projects", workspaceId, documentId] });
      await queryClient.invalidateQueries({ queryKey: ["projects", workspaceId] });
    },
  });

  const createSurface = originCreateSurface({
    isLoading: projects.isLoading,
    isError: projects.isError,
    itemCount: projects.data?.items.length,
    canCreateProject: projects.data?.canCreateProject,
  });

  if (hideWhenEmpty && !origins.isLoading && !origins.isError && origins.data?.count === 0) return null;
  return (
    <section aria-label={documentId ? t("collection.linkedTasks") : t("collection.sourceDocument")} className="flex flex-col gap-3 rounded-md border border-border p-4">
      <h2 className="text-title">{documentId ? t("collection.linkedTasks") : t("collection.sourceDocument")} ({origins.data?.count ?? 0})</h2>
      {origins.isLoading ? <p role="status">{t("collection.origins.loading")}</p> : null}
      {origins.isError ? <p role="alert">{errorText(origins.error, t("collection.origins.error"))}</p> : null}
      {origins.data?.items.map((item) => (
        <Link key={item.taskId} className="text-ui underline" to={itemPath(slug, documentId ? item.taskDisplayId : item.documentDisplayId)}>
          {documentId ? `${item.taskDisplayId} · ${item.taskTitle}` : `${item.documentDisplayId} · ${item.documentTitle}`}
        </Link>
      ))}
      {origins.data && origins.data.count === 0 ? <p>{t("collection.noOrigins")}</p> : null}
      {after ? <Button type="button" variant="outline" onClick={() => setAfter(null)}>{t("collection.origins.first")}</Button> : null}
      {origins.data?.nextCursor ? <Button type="button" variant="outline" onClick={() => setAfter(origins.data!.nextCursor ?? null)}>{t("collection.origins.next")}</Button> : null}
      {documentId ? (
        <div className="flex flex-col gap-3">
          {createSurface === "loading" ? <p role="status">{t("collection.taskCreation.projectsLoading")}</p> : null}
          {createSurface === "error" ? <p role="alert">{errorText(projects.error, t("collection.taskCreation.projectsError"))}</p> : null}
          {createSurface === "create-task" && projects.data ? (
            <form className="flex flex-wrap items-end gap-2" onKeyDown={preventImeSubmit} onSubmit={(event) => { event.preventDefault(); if (projectId && title.trim()) createTask.mutate(); }}>
              <div><Label htmlFor={`origin-project-${documentId}`}>{t("collection.taskProject")}</Label>
                <select id={`origin-project-${documentId}`} className="h-10 rounded-md border border-input bg-background px-2" value={projectId} onChange={(event) => { setProjectId(event.target.value); setRequestId(crypto.randomUUID()); }}>
                  {projects.data.items.map((project) => <option key={project.id} value={project.id}>{project.name} ({project.key})</option>)}
                </select>
              </div>
              <div><Label htmlFor={`origin-title-${documentId}`}>{t("collection.taskTitle")}</Label><Input id={`origin-title-${documentId}`} value={title} onChange={(event) => { setTitle(event.target.value); setRequestId(crypto.randomUUID()); }} /></div>
              <Button type="submit" disabled={!projectId || !title.trim() || createTask.isPending}>{t("collection.createTask")}</Button>
            </form>
          ) : null}
          {createSurface === "create-project" ? (
            <form className="flex flex-wrap items-end gap-2" onKeyDown={preventImeSubmit} onSubmit={(event) => { event.preventDefault(); if (projectName.trim() && projectKey.trim()) createProject.mutate(); }}>
              <p className="w-full">{t("collection.projectRequired")}</p>
              <div><Label htmlFor={`origin-project-name-${documentId}`}>{t("collection.projectName")}</Label><Input id={`origin-project-name-${documentId}`} value={projectName} onChange={(event) => setProjectName(event.target.value)} /></div>
              <div><Label htmlFor={`origin-project-key-${documentId}`}>{t("project.keyLabel")}</Label><Input id={`origin-project-key-${documentId}`} value={projectKey} onChange={(event) => setProjectKey(event.target.value)} /></div>
              <Button type="submit" disabled={!projectName.trim() || !projectKey.trim() || createProject.isPending}>{t("project.new")}</Button>
            </form>
          ) : null}
          {createSurface === "unavailable" ? <p>{t("collection.taskCreation.unavailable")}</p> : null}
          {createTask.isError ? <p role="alert">{errorText(createTask.error, t("collection.taskCreation.error"))}</p> : null}
          {createProject.isError ? <p role="alert">{errorText(createProject.error, t("collection.taskCreation.projectError"))}</p> : null}
        </div>
      ) : null}
    </section>
  );
}
