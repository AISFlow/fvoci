// Ported from source apps/web/src/features/workspace/backlinks-list.tsx and the
// task detail screen's backlinks query: shown only when something links here.
import { t } from "@fvoci/i18n";
import { useQuery } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import { api, ensureOk } from "@/lib/api";
import { itemPath } from "@/lib/href";

export function TaskBacklinks({
  slug,
  workspaceId,
  taskId,
}: {
  slug: string;
  workspaceId: string;
  taskId: string;
}) {
  const backlinks = useQuery({
    queryKey: ["backlinks", "task", workspaceId, taskId] as const,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/backlinks", {
          params: { path: { workspace_id: workspaceId, task_id: taskId } },
        }),
      ),
    enabled: Boolean(workspaceId) && Boolean(taskId),
    retry: false,
  });
  const items = backlinks.data?.items ?? [];
  if (items.length === 0) return null;
  return (
    <section className="flex flex-col gap-2" data-testid="task-backlinks">
      <h2 className="text-doc font-medium">{t("backlinks.title")}</h2>
      <ul className="flex flex-col gap-1">
        {items.map((item) => (
          <li key={item.id}>
            {item.from.displayId ? (
              <Link to={itemPath(slug, item.from.displayId)} className="break-keep hover:underline">
                {item.from.title}
              </Link>
            ) : (
              <span className="break-keep">{item.from.title}</span>
            )}
          </li>
        ))}
      </ul>
    </section>
  );
}
