import { t } from "@fvoci/i18n";
import { useId, useState } from "react";
import { Link } from "react-router-dom";
import { EmptyState } from "@/components/empty-state";
import { QueryError, QueryLoading } from "@/components/query-status";
import { Button } from "@/components/ui/button";
import { projectTasksPath } from "@/lib/href";
import { CreateProjectForm } from "./create-project-form";
import { NativeModal } from "./native-modal";
import "./projects.css";
import type { CreateProjectBody, ProjectListItem } from "./queries";

interface ProjectsViewProps {
  slug: string;
  projects: ProjectListItem[];
  loading: boolean;
  error: string | null;
  creating?: boolean;
  onRetry: () => void;
  onCreate: (input: CreateProjectBody) => Promise<void>;
}

export function ProjectsView({
  slug,
  projects,
  loading,
  error,
  creating,
  onRetry,
  onCreate,
}: ProjectsViewProps) {
  const titleId = useId();
  const [createOpen, setCreateOpen] = useState(false);
  const active = projects.filter((project) => project.status === "active");

  return (
    <div className="project-home">
      <div className="project-home__head">
        <h1 className="project-home__title">{t("nav.projects")}</h1>
        {projects.length > 0 ? (
          <Button type="button" onClick={() => setCreateOpen(true)}>
            {t("project.new")}
          </Button>
        ) : null}
      </div>
      {loading ? <QueryLoading /> : null}
      {!loading && error ? <QueryError message={error} onRetry={onRetry} /> : null}
      {!loading && !error && active.length === 0 ? (
        <EmptyState
          title={projects.length === 0 ? t("project.emptyHint") : t("project.emptyArchived")}
          actionLabel={t("project.new")}
          onAction={() => setCreateOpen(true)}
          actionDisabled={creating}
        />
      ) : null}
      {!loading && !error && active.length > 0 ? (
        <ul className="project-list">
          {active.map((project) => (
            <li key={project.id}>
              <Link to={projectTasksPath(slug, project.key)} className="project-list__row">
                <span className="project-list__key">{project.key}</span>
                <span className="project-list__meta">
                  <span className="project-list__name">{project.name}</span>
                  {project.visibility === "private" ? (
                    <span className="project-list__private">{t("project.visibility.private")}</span>
                  ) : null}
                </span>
                <span className="project-list__count">
                  {t("entrance.openTaskCount", { count: project.openTaskCount })}
                </span>
              </Link>
            </li>
          ))}
        </ul>
      ) : null}
      <NativeModal open={createOpen} labelledBy={titleId} onClose={() => setCreateOpen(false)}>
        <h2 id={titleId} className="project-dialog__title">
          {t("project.new")}
        </h2>
        <CreateProjectForm
          pending={creating}
          onSubmit={async (input) => {
            await onCreate(input);
            setCreateOpen(false);
          }}
          onCancel={() => setCreateOpen(false)}
        />
      </NativeModal>
    </div>
  );
}
