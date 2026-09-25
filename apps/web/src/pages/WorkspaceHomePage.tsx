import { t } from "@fvoci/i18n";
import { useQuery } from "@tanstack/react-query";
import { useId } from "react";
import { Link } from "react-router-dom";
import { QueryError, QueryLoading, loadErrorMessage } from "@/components/query-status";
import { projectsQuery } from "@/features/projects/queries";
import { WorkspaceShell } from "@/features/workspace/workspace-shell";
import { useWorkspaceContext } from "@/hooks/use-workspace-context";
import { itemPath } from "@/lib/href";
import { recentQuery, starsQuery } from "@/lib/queries/share";
import { starItemDisplayId } from "@/lib/share-links";
import "@/features/share/share.css";

const dayFormat = new Intl.DateTimeFormat("ko", { month: "numeric", day: "numeric" });

type EntranceItem = {
  key: string;
  type: string;
  projectId: string | null;
  number: number;
  title: string;
  meta?: string;
};

function EntranceSection({
  slug,
  heading,
  empty,
  items,
  loading,
  error,
  onRetry,
  projectKeyById,
}: {
  slug: string;
  heading: string;
  empty: string;
  items: readonly EntranceItem[];
  loading: boolean;
  error: string | null;
  onRetry: () => void;
  projectKeyById: ReadonlyMap<string, string>;
}) {
  const headingId = useId();
  return (
    <section className="entrance__section" aria-labelledby={headingId}>
      <h2 id={headingId} className="entrance__heading">
        {heading}
      </h2>
      {loading ? <QueryLoading /> : null}
      {!loading && error ? <QueryError message={error} onRetry={onRetry} /> : null}
      {!loading && !error && items.length === 0 ? (
        <p className="entrance__empty">{empty}</p>
      ) : null}
      {items.length > 0 ? (
        <ul className="entrance__list">
          {items.map((item) => {
            const displayId = starItemDisplayId(item, projectKeyById);
            const row = (
              <>
                <span className="entrance__key">{displayId ?? "—"}</span>
                <span className="entrance__title">{item.title}</span>
                <span className="entrance__meta">{item.meta ?? ""}</span>
              </>
            );
            return (
              <li key={item.key}>
                {displayId ? (
                  <Link className="entrance__row" to={itemPath(slug, displayId)}>
                    {row}
                  </Link>
                ) : (
                  <span className="entrance__row">{row}</span>
                )}
              </li>
            );
          })}
        </ul>
      ) : null}
    </section>
  );
}

/** Source `workspace-entrance` rail: starred items and recently updated items. */
export function WorkspaceHomePage() {
  const { slug, workspace } = useWorkspaceContext();
  const workspaceId = workspace?.id ?? "";
  const stars = useQuery(starsQuery(workspaceId));
  const recent = useQuery(recentQuery(workspaceId, 8));
  const projects = useQuery(projectsQuery(workspaceId));

  if (!workspace) return null;

  const projectKeyById = new Map(
    (projects.data?.items ?? []).map((project) => [project.id, project.key] as const),
  );

  return (
    <WorkspaceShell
      slug={slug}
      workspaceId={workspace.id}
      workspaceName={workspace.name}
      activeNav="home"
    >
      <div className="entrance">
        <h1 className="wiki-home__title">{workspace.name}</h1>
        <EntranceSection
          slug={slug}
          heading={t("star.home")}
          empty={t("entrance.stars.emptyHint")}
          items={(stars.data?.items ?? []).map((star) => ({
            key: `${star.type}:${star.targetId}`,
            type: star.type,
            projectId: star.projectId,
            number: star.number,
            title: star.title,
          }))}
          loading={stars.isLoading}
          error={stars.isError ? loadErrorMessage(stars.error) : null}
          onRetry={() => {
            void stars.refetch();
          }}
          projectKeyById={projectKeyById}
        />
        <EntranceSection
          slug={slug}
          heading={t("recent.title")}
          empty={t("recent.empty")}
          items={(recent.data?.items ?? []).map((item) => ({
            key: `${item.type}:${item.id}`,
            type: item.type,
            projectId: item.projectId,
            number: item.number,
            title: item.title,
            meta: dayFormat.format(new Date(item.updatedAt)),
          }))}
          loading={recent.isLoading}
          error={recent.isError ? loadErrorMessage(recent.error) : null}
          onRetry={() => {
            void recent.refetch();
          }}
          projectKeyById={projectKeyById}
        />
      </div>
    </WorkspaceShell>
  );
}
