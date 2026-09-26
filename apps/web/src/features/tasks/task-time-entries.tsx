// Ported from source apps/web/src/features/tasks/task-time-entries.tsx:
// list with total, and a closed-range entry form in the viewer's time zone.
import { formatPersonName, t } from "@fvoci/i18n";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { type FormEvent, useState } from "react";
import { QueryError, QueryLoading, loadErrorMessage } from "@/components/query-status";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import type { components } from "@/generated/api";
import { api, ensureOk, ProblemError } from "@/lib/api";
import type { MemberOutput } from "@/lib/contracts";
import {
  datetimeLocalInTimeZoneToIso,
  durationSecondsBetween,
  FALLBACK_TZ,
  formatInstant,
} from "@/lib/datetime";
import { meQuery } from "@/lib/queries";
import { formatDuration } from "./time-entry-format";

type TimeEntry = components["schemas"]["TimeEntryOutput"];
type TimeEntryCreateBody = components["schemas"]["TimeEntryCreateBody"];

export function taskTimeEntriesQueryKey(workspaceId: string, taskId: string) {
  return ["task-time-entries", workspaceId, taskId] as const;
}

function memberName(members: readonly MemberOutput[], userId: string): string {
  const member = members.find((m) => m.userId === userId);
  return member ? formatPersonName(member) : userId.slice(0, 8);
}

function TimeEntryForm({
  timeZone,
  pending,
  formError,
  onCreate,
  onCancel,
}: {
  timeZone: string;
  pending: boolean;
  formError: string | null;
  onCreate: (body: TimeEntryCreateBody) => Promise<void>;
  onCancel: () => void;
}) {
  const [startedLocal, setStartedLocal] = useState("");
  const [endedLocal, setEndedLocal] = useState("");
  const [note, setNote] = useState("");
  const [localError, setLocalError] = useState<string | null>(null);
  const shownError = localError ?? formError;

  function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (startedLocal === "" && endedLocal === "") return;
    if (startedLocal === "" || endedLocal === "") {
      setLocalError(t("task.time.duration.needRange"));
      return;
    }
    const startedAt = datetimeLocalInTimeZoneToIso(startedLocal, timeZone);
    const endedAt = datetimeLocalInTimeZoneToIso(endedLocal, timeZone);
    const span = durationSecondsBetween(startedAt, endedAt);
    if (startedAt === "" || endedAt === "" || !Number.isFinite(span) || span <= 0) {
      setLocalError(t("task.time.duration.invalid"));
      return;
    }
    const body: TimeEntryCreateBody = { startedAt, endedAt };
    const trimmed = note.trim();
    if (trimmed !== "") body.note = trimmed;
    setLocalError(null);
    void onCreate(body);
  }

  return (
    <form className="flex flex-col gap-2 sm:max-w-lg" noValidate onSubmit={onSubmit}>
      <div className="grid gap-2 sm:grid-cols-2">
        <div className="flex flex-col gap-1">
          <label className="text-ui text-muted-foreground" htmlFor="task-time-started">
            {t("task.time.startedAt")}
          </label>
          <Input
            id="task-time-started"
            type="datetime-local"
            value={startedLocal}
            onChange={(e) => setStartedLocal(e.target.value)}
          />
        </div>
        <div className="flex flex-col gap-1">
          <label className="text-ui text-muted-foreground" htmlFor="task-time-ended">
            {t("task.time.endedAt")}
          </label>
          <Input
            id="task-time-ended"
            type="datetime-local"
            value={endedLocal}
            onChange={(e) => setEndedLocal(e.target.value)}
          />
        </div>
      </div>
      <div className="flex flex-col gap-1">
        <label className="text-ui text-muted-foreground" htmlFor="task-time-note">
          {t("task.time.note")}
        </label>
        <textarea
          id="task-time-note"
          className="min-h-16 rounded-md border border-border bg-background px-3 py-2 text-doc"
          maxLength={2000}
          value={note}
          onChange={(e) => setNote(e.target.value)}
        />
      </div>
      {shownError ? (
        <p role="alert" className="break-keep text-ui text-destructive">
          {shownError}
        </p>
      ) : null}
      <div className="flex flex-wrap gap-2">
        <Button type="submit" size="sm" disabled={pending}>
          {t("task.time.submit")}
        </Button>
        <Button type="button" variant="outline" size="sm" onClick={onCancel}>
          {t("task.create.cancel")}
        </Button>
      </div>
    </form>
  );
}

export function TaskTimeEntries({
  workspaceId,
  taskId,
  members,
  readOnly,
}: {
  workspaceId: string;
  taskId: string;
  members: readonly MemberOutput[];
  readOnly: boolean;
}) {
  const queryClient = useQueryClient();
  const queryKey = taskTimeEntriesQueryKey(workspaceId, taskId);
  const list = useQuery({
    queryKey,
    queryFn: async () =>
      ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/time-entries", {
          params: { path: { workspace_id: workspaceId, task_id: taskId } },
        }),
      ),
    enabled: Boolean(workspaceId) && Boolean(taskId),
    retry: false,
  });
  const me = useQuery(meQuery);
  const timeZone = me.data?.timezone ?? FALLBACK_TZ;
  const [formOpen, setFormOpen] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);
  const create = useMutation({
    mutationFn: async (body: TimeEntryCreateBody) =>
      ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/time-entries", {
          params: { path: { workspace_id: workspaceId, task_id: taskId } },
          body,
        }),
      ),
    onSuccess: async () => {
      setFormError(null);
      setFormOpen(false);
      await queryClient.invalidateQueries({ queryKey });
    },
    onError: (err) => {
      setFormError(err instanceof ProblemError ? err.title : t("load.failed"));
    },
  });

  const items: TimeEntry[] = list.data?.items ?? [];
  const canCreate = !readOnly && list.isSuccess && list.data.canCreate;
  const total = items.reduce((sum, row) => sum + (row.durationSeconds ?? 0), 0);
  const showHeading = !list.isSuccess || items.length > 0 || formOpen;

  return (
    <section className="flex flex-col gap-2 text-doc" data-testid="task-time-entries">
      {showHeading ? <h2 className="text-ui font-medium">{t("task.time.heading")}</h2> : null}
      {list.isLoading ? <QueryLoading /> : null}
      {list.isError ? (
        <QueryError
          message={loadErrorMessage(list.error)}
          onRetry={() => {
            void list.refetch();
          }}
        />
      ) : null}
      {items.length > 0 ? (
        <div className="flex flex-col gap-1.5">
          <p className="break-keep text-ui" data-testid="task-time-total">
            {t("task.time.total")} {formatDuration(total)}
          </p>
          <ul className="flex flex-col gap-1">
            {items.map((row) => (
              <li className="break-keep text-doc" key={row.id}>
                {memberName(members, row.userId)}{" "}
                {formatInstant(row.startedAt, timeZone, {
                  month: "numeric",
                  day: "numeric",
                  hour: "2-digit",
                  minute: "2-digit",
                })}{" "}
                {row.durationSeconds == null ? t("task.time.open") : formatDuration(row.durationSeconds)}
                {row.note ? ` · ${row.note}` : null}
              </li>
            ))}
          </ul>
        </div>
      ) : null}
      {canCreate && !formOpen ? (
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="self-start"
          aria-expanded={false}
          onClick={() => setFormOpen(true)}
        >
          {t("task.time.submit")}
        </Button>
      ) : null}
      {canCreate && formOpen ? (
        <TimeEntryForm
          timeZone={timeZone}
          pending={create.isPending}
          formError={formError}
          onCancel={() => {
            setFormOpen(false);
            setFormError(null);
          }}
          onCreate={async (body) => {
            try {
              await create.mutateAsync(body);
            } catch {
              /* onError keeps the form open with the problem title. */
            }
          }}
        />
      ) : null}
    </section>
  );
}
