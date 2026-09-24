import { createElement } from "react";
import { t } from "@fvoci/i18n";

export function TaskListLoadMore({
  error,
  pending,
  onLoadMore,
}: {
  error?: string | null;
  pending?: boolean;
  onLoadMore: () => void;
}) {
  return createElement(
    "div",
    { className: "flex flex-col items-start gap-2" },
    error
      ? createElement(
          "p",
          { role: "alert", className: "task-form__alert" },
          error,
        )
      : null,
    createElement(
      "button",
      {
        type: "button",
        className:
          "inline-flex h-10 items-center justify-center rounded-md border border-border bg-background px-4 text-ui font-medium hover:bg-accent hover:text-foreground disabled:pointer-events-none disabled:opacity-50",
        disabled: pending,
        onClick: onLoadMore,
      },
      pending ? t("load.loading") : t("task.list.loadMore"),
    ),
  );
}
