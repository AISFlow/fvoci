import { t } from "@fvoci/i18n";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { api, ensureOk } from "@/lib/api";
import "./settings-shell.css";

async function copyText(value: string): Promise<void> {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(value);
    return;
  }
  throw new Error("clipboard unavailable");
}

export function WorkspaceCalendarSection({ workspaceId }: { workspaceId: string }) {
  const client = useQueryClient();
  const holidays = useQuery({
    queryKey: ["workspaces", workspaceId, "holidays"],
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/holidays", {
          params: { path: { workspace_id: workspaceId } },
        }),
      ),
    retry: false,
  });
  const canEdit = holidays.data?.canEdit;
  const copyBusy = useRef(false);
  const writeBusy = useRef(false);
  const [date, setDate] = useState("");
  const url = useRef<string | null>(null);
  const copy = useMutation({
    mutationFn: async () => {
      url.current ??= (
        await ensureOk(
          await api.POST("/api/v1/workspaces/{workspace_id}/ics-token", {
            params: { path: { workspace_id: workspaceId } },
          }),
        )
      ).url;
      await copyText(url.current);
    },
    onSettled: () => {
      copyBusy.current = false;
    },
  });
  const write = useMutation({
    mutationFn: async (input: { date: string; remove: boolean }) =>
      input.remove
        ? ensureOk(
            await api.DELETE("/api/v1/workspaces/{workspace_id}/holidays/{date}", {
              params: { path: { workspace_id: workspaceId, date: input.date } },
            }),
          )
        : ensureOk(
            await api.POST("/api/v1/workspaces/{workspace_id}/holidays", {
              params: { path: { workspace_id: workspaceId } },
              body: { date: input.date },
            }),
          ),
    onSuccess: async (_result, input) => {
      client.setQueryData(
        ["workspaces", workspaceId, "holidays"],
        (current: { canEdit?: boolean; items?: string[] } | undefined) => ({
          canEdit: current?.canEdit ?? false,
          items: input.remove
            ? (current?.items ?? []).filter((day) => day !== input.date)
            : [...new Set([...(current?.items ?? []), input.date])].sort(),
        }),
      );
      setDate("");
      await client.invalidateQueries({
        queryKey: ["workspaces", workspaceId, "holidays"],
      });
    },
    onSettled: () => {
      writeBusy.current = false;
    },
  });
  const writesDisabled = holidays.isPending || holidays.isError || write.isPending;
  function changeHoliday(input: { date: string; remove: boolean }) {
    if (writesDisabled || writeBusy.current) return;
    writeBusy.current = true;
    write.mutate(input);
  }
  return (
    <section className="settings-section">
      <details className="rounded-md border p-3">
        <summary className="min-h-11 cursor-pointer">
          {t("ics.subscribe")} · {t("ics.holidays")}
        </summary>
        <div className="flex flex-col gap-2">
          <Button
            type="button"
            disabled={copy.isPending}
            onClick={() => {
              if (copyBusy.current) return;
              copyBusy.current = true;
              copy.mutate();
            }}
          >
            {t(copy.isSuccess ? "ics.subscribe.copied" : "ics.subscribe.copy")}
          </Button>
          {copy.isError ? <p role="alert">{t("ics.subscribe.failed")}</p> : null}
          {holidays.isPending ? (
            <p role="status">{t("load.loading")}</p>
          ) : holidays.data?.items.length === 0 ? (
            <p>{t("ics.holidays.empty")}</p>
          ) : (
            <ul>
              {holidays.data?.items.map((day) => (
                <li key={day} className="flex items-center justify-between gap-2">
                  <time>{day}</time>
                  {canEdit ? (
                    <Button
                      type="button"
                      variant="outline"
                      disabled={writesDisabled}
                      onClick={() => changeHoliday({ date: day, remove: true })}
                    >
                      {day} {t("ics.holidays.remove")}
                    </Button>
                  ) : null}
                </li>
              ))}
            </ul>
          )}
          {canEdit ? (
            <form
              className="flex flex-wrap gap-2"
              onSubmit={(e) => {
                e.preventDefault();
                if (date) changeHoliday({ date, remove: false });
              }}
            >
              <Input
                type="date"
                aria-label={t("ics.holidays.date")}
                value={date}
                onChange={(e) => setDate(e.target.value)}
              />
              <Button type="submit" disabled={!date || writesDisabled}>
                {t("ics.holidays.add")}
              </Button>
            </form>
          ) : null}
          {holidays.isError ? (
            <div role="alert">
              <p>
                {t(
                  write.isSuccess
                    ? "ics.holidays.refreshFailed"
                    : "ics.holidays.loadFailed",
                )}
              </p>
              <Button type="button" onClick={() => void holidays.refetch()}>
                {t("load.retry")}
              </Button>
            </div>
          ) : null}
          {write.isSuccess ? <p role="status">{t("ics.holidays.saved")}</p> : null}
          {write.isError ? <p role="alert">{t("ics.holidays.writeFailed")}</p> : null}
        </div>
      </details>
    </section>
  );
}
