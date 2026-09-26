import { t } from "@fvoci/i18n";
import { useInfiniteQuery } from "@tanstack/react-query";
import { useDeferredValue, useEffect, useMemo, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { taskParentListQuery } from "./queries";

export function TaskParentSelect({
  workspaceId,
  projectId,
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
  const q = useDeferredValue(query).trim();

  const list = useInfiniteQuery({
    ...taskParentListQuery(workspaceId, projectId, childType, excludeTaskId, q),
    enabled: open && Boolean(workspaceId) && Boolean(projectId) && childType !== "epic",
  });

  useEffect(() => {
    setOpen(false);
    setQuery("");
    setSelected(null);
  }, [childType, excludeTaskId]);

  const items = useMemo(
    () => list.data?.pages.flatMap((page) => page.items) ?? [],
    [list.data],
  );

  const label = value
    ? selected?.id === value
      ? selected.title
      : (currentTitle ?? t("task.parent.current"))
    : t("task.parent.none");
  const listPending = open && list.isPending;
  const listError = open ? list.error : null;
  const showEmpty = items.length === 0 && !list.hasNextPage;

  return (
    <div className="task-parent-select">
      <Button
        type="button"
        variant="outline"
        className="task-parent-select__trigger"
        id="task-edit-parent"
        data-testid="task-edit-parent"
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
            autoFocus
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            aria-label={t("task.parent.search")}
            placeholder={t("task.parent.search")}
            data-testid="task-edit-parent-search"
            disabled={disabled}
          />
          {listPending ? (
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
                  void list.refetch();
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
              aria-busy={list.isFetching}
            >
              {childType !== "subtask" ? (
                <li role="presentation">
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
                const text = `${item.displayId} ${item.title}`;
                return (
                  <li key={item.id} role="presentation">
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
              {showEmpty ? (
                <li role="presentation" className="task-home__note">
                  {t("task.parent.empty")}
                </li>
              ) : null}
            </ul>
          )}
          {list.hasNextPage ? (
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
