import { t } from "@fvoci/i18n";
import { useId, useState } from "react";
import { Link } from "react-router-dom";
import { EmptyState } from "@/components/empty-state";
import { QueryError, QueryLoading } from "@/components/query-status";
import { Button } from "@/components/ui/button";
import { CloneProjectForm } from "./clone-project-form";
import { CreateProjectForm } from "./create-project-form";
import { NativeModal } from "./native-modal";
import "./projects.css";
import type { CloneProjectBody, CreateProjectBody, ProjectListItem } from "./queries";
import type { components } from "@/generated/api";
import { projectPath } from "@/lib/href";

type Member = components["schemas"]["MemberResponse"];

interface ProjectsViewProps {
  slug: string;
  projects: ProjectListItem[];
  members: readonly Member[];
  currentUserId: string | null;
  loading: boolean;
  error: string | null;
  creating?: boolean;
  cloning?: boolean;
  onRetry: () => void;
  onCreate: (input: CreateProjectBody) => Promise<void>;
  onClone: (projectId: string, input: CloneProjectBody) => Promise<void>;
}

export function ProjectsView({
  slug,
  projects,
  loading,
  error,
  members,
  currentUserId,
  creating,
  cloning,
  onRetry,
  onCreate,
  onClone,
}: ProjectsViewProps) {
  const titleId = useId();
  const cloneTitleId = useId();
  const [createOpen, setCreateOpen] = useState(false);
  const [cloneSource, setCloneSource] = useState<ProjectListItem | null>(null);
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
              <Link to={projectPath(slug, project.key)} className="project-list__row">
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
                <Button
                  type="button"
                  variant="outline"
                  className="project-list__clone"
                  onClick={(event) => {
                    event.preventDefault();
                    setCloneSource(project);
                  }}
                >
                  {t("project.clone")}
                </Button>
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
          members={members}
          currentUserId={currentUserId}
          onSubmit={async (input) => {
            await onCreate(input);
            setCreateOpen(false);
          }}
          onCancel={() => setCreateOpen(false)}
        />
      </NativeModal>
      <NativeModal
        open={cloneSource !== null}
        labelledBy={cloneTitleId}
        onClose={() => setCloneSource(null)}
      >
        {cloneSource ? (
          <>
            <h2 id={cloneTitleId} className="project-dialog__title">
              {t("project.clone.title")}
            </h2>
            <CloneProjectForm
              source={cloneSource}
              pending={cloning}
              members={members}
              currentUserId={currentUserId}
              onSubmit={async (input) => {
                await onClone(cloneSource.id, input);
                setCloneSource(null);
              }}
              onCancel={() => setCloneSource(null)}
            />
          </>
        ) : null}
      </NativeModal>
    </div>
  );
}
