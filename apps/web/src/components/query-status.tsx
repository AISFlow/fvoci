import { t } from "@fvoci/i18n";
import { Button } from "@/components/ui/button";
import { ProblemError } from "@/lib/api";

export function loadErrorMessage(error: unknown): string {
  return error instanceof ProblemError ? error.title : t("load.failed");
}

export function QueryLoading() {
  return (
    <p role="status" className="py-4 text-ui text-muted-foreground">
      {t("load.loading")}
    </p>
  );
}

export function QueryError({
  message,
  onRetry,
}: {
  message: string;
  onRetry: () => void;
}) {
  return (
    <div
      role="alert"
      className="flex flex-wrap items-center justify-between gap-3 rounded-lg border border-destructive/25 bg-destructive/5 p-4"
    >
      <p className="break-keep text-ui text-destructive">{message}</p>
      <Button type="button" variant="outline" size="sm" className="w-fit" onClick={onRetry}>
        {t("load.retry")}
      </Button>
    </div>
  );
}
