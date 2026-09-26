// Source `ProjectViewChrome` tabs: task list, collection views and field settings.
import { t } from "@fvoci/i18n";
import { Link } from "react-router-dom";
import {
  projectCollectionPath,
  projectFieldsPath,
  projectTasksPath,
  projectWorkflowPath,
} from "@/lib/href";
import "./collections.css";

export type ProjectViewTab = "tasks" | "table" | "board" | "calendar" | "fields" | "workflow";

export function ProjectViewNav({
  slug,
  projectKey,
  active,
}: {
  slug: string;
  projectKey: string;
  active: ProjectViewTab;
}) {
  const tabs: Array<{ id: ProjectViewTab; to: string; label: string }> = [
    { id: "tasks", to: projectTasksPath(slug, projectKey), label: t("nav.tasks") },
    { id: "table", to: projectCollectionPath(slug, projectKey, "table"), label: t("collection.table") },
    { id: "board", to: projectCollectionPath(slug, projectKey, "board"), label: t("collection.board") },
    {
      id: "calendar",
      to: projectCollectionPath(slug, projectKey, "calendar"),
      label: t("collection.calendar"),
    },
    { id: "fields", to: projectFieldsPath(slug, projectKey), label: t("collection.fieldSettings") },
    {
      id: "workflow",
      to: projectWorkflowPath(slug, projectKey),
      label: t("project.settings.workflow"),
    },
  ];
  return (
    <nav className="project-view-nav" aria-label={t("project.settings.navigation")}>
      {tabs.map((tab) => (
        <Link
          key={tab.id}
          to={tab.to}
          className={tab.id === active ? "project-view-nav__link is-active" : "project-view-nav__link"}
          aria-current={tab.id === active ? "page" : undefined}
        >
          {tab.label}
        </Link>
      ))}
    </nav>
  );
}
