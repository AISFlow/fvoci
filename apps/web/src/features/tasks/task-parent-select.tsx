import { t } from "@fvoci/i18n";
import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { useDeferredValue, useEffect, useMemo, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { formatDisplayId } from "@/lib/href";
import { lookupQuery } from "./lookup";
import { taskParentListQuery } from "./queries";
import { eligibleParentCandidates } from "./task-edit-payload";
import { parentSearchMode } from "./task-parent-query";

type ParentCandidate = {
  id: string;
  type: string;
  number: number;
  title: string;
  displayId?: string;
};

export function TaskParentSelect({
  workspaceId,
  projectId,
  projectKey,
  childType,
  excludeTaskId,
  value,
  currentTitle,
  onChange,
  disabled,
}: {
  workspaceId: string;
  projectId: string;
  projectKey: string;
  childType: string;
  excludeTaskId: string;
  value: string | null;
  currentTitle?: string;
  onChange: (value: string | null) => void;
  disabled?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<{ id: string; title: string } | null>(null);
  const q = useDeferredValue(query);
  const mode = parentSearchMode(q, projectKey);
  const title = mode.kind === "list" ? mode.title : undefined;
  const displayId = mode.kind === "display-id" ? mode.displayId : "";
  const listEnabled = open && mode.kind === "list";
  const lookupEnabled = open && mode.kind === "display-id";

  const list = useInfiniteQuery({
    ...taskParentListQuery(workspaceId, projectId, childType, excludeTaskId, title),
    enabled:
      listEnabled &&
      Boolean(workspaceId) &&
      Boolean(projectId) &&
      childType !== "epic",
  });
  const lookup = useQuery({
    ...lookupQuery(workspaceId, displayId),
    enabled: lookupEnabled && Boolean(workspaceId) && Boolean(displayId),
  });

  useEffect(() => {
    setOpen(false);
    setQuery("");
    setSelected(null);
  }, [childType, excludeTaskId]);

  const items = useMemo((): ParentCandidate[] => {
    if (mode.kind === "empty") return [];
    if (mode.kind === "display-id") {
      return (lookup.data?.items ?? [])
        .filter(
          (item) =>
            item.kind === "task" &&
            item.id !== excludeTaskId &&
            item.projectId === projectId,
        )
        .map((item) => ({
          id: item.id,
          type: "task",
          number: 0,
          title: item.title,
          displayId: item.displayId,
        }));
    }
    const pages = list.data?.pages.flatMap((page) => page.items) ?? [];
    return eligibleParentCandidates({ id: excludeTaskId, type: childType }, pages);
  }, [
    childType,
    excludeTaskId,
    list.data,
    lookup.data,
    mode.kind,
    projectId,
  ]);

  const label = value
    ? selected?.id === value
      ? selected.title
      : (currentTitle ?? t("task.parent.current"))
    : t("task.parent.none");
  const listPending = listEnabled && list.isPending;
  const lookupPending = lookupEnabled && lookup.isPending;
  const listError = listEnabled ? list.error : lookupEnabled ? lookup.error : null;

  return (
    <div className="task-parent-select">
      <Button
        type="button"
        variant="outline"
        className="task-parent-select__trigger"
        id="task-edit-parent"
        data-testid="task-edit-parent"
        aria-label={t("task.parent.label")}
        aria-expanded={open}
        aria-haspopup="listbox"
        disabled={disabled || childType === "epic" || !workspaceId || !projectId}
        onClick={() => setOpen((current) => !current)}
      >
        {label}
      </Button>
      {open ? (
        <div className="task-parent-select__panel">
          <Input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            aria-label={t("task.parent.search")}
            placeholder={t("task.parent.search")}
            data-testid="task-edit-parent-search"
            disabled={disabled}
          />
          {listPending || lookupPending ? (
            <p role="status" className="task-home__note">
              {t("task.parent.loading")}
            </p>
          ) : listError ? (
            <div role="alert">
              <p className="task-form__alert">{t("task.parent.failed")}</p>
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => {
                  if (mode.kind === "list") void list.refetch();
                  else void lookup.refetch();
                }}
              >
                {t("task.parent.retry")}
              </Button>
            </div>
          ) : (
            <ul
              className="task-parent-select__list"
              role="listbox"
              aria-label={t("task.parent.label")}
              aria-busy={list.isFetching || lookup.isFetching}
            >
              {childType !== "subtask" ? (
                <li>
                  <button
                    type="button"
                    role="option"
                    className="task-parent-select__option"
                    aria-selected={value == null}
                    data-testid="task-edit-parent-none"
                    onClick={() => {
                      onChange(null);
                      setSelected(null);
                      setOpen(false);
                    }}
                  >
                    {t("task.parent.none")}
                  </button>
                </li>
              ) : null}
              {items.map((item) => {
                const text = `${item.displayId ?? formatDisplayId(projectKey, item.number)} ${item.title}`;
                return (
                  <li key={item.id}>
                    <button
                      type="button"
                      role="option"
                      className="task-parent-select__option"
                      aria-selected={value === item.id}
                      data-testid={`task-edit-parent-option-${item.id}`}
                      onClick={() => {
                        setSelected({ id: item.id, title: text });
                        onChange(item.id);
                        setOpen(false);
                      }}
                    >
                      {text}
                    </button>
                  </li>
                );
              })}
              {items.length === 0 ? (
                <li className="task-home__note">{t("task.parent.empty")}</li>
              ) : null}
            </ul>
          )}
          {mode.kind === "list" && list.hasNextPage ? (
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={list.isFetchingNextPage}
              onClick={() => void list.fetchNextPage()}
            >
              {t("task.parent.more")}
            </Button>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
