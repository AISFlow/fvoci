import { t } from "@fvoci/i18n";
import { useMutation } from "@tanstack/react-query";
import { useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { ProblemError } from "@/lib/api";
import { isImportActive, pollImportJob, type ImportJobStatus } from "@/lib/import-poll";
import "../settings/settings-shell.css";

type ImportSource = "markdown-zip" | "office-file" | "notion-zip";

const IMPORT_ACCEPT: Record<ImportSource, string> = {
  "markdown-zip": ".zip,application/zip",
  "office-file": ".pdf,.docx,.pptx,.xlsx,.odt,.odp,.ods,.hwp,.hwpx",
  "notion-zip": ".zip,application/zip",
};

function fileToBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const result = reader.result;
      if (typeof result !== "string") {
        reject(new Error("read failed"));
        return;
      }
      const comma = result.indexOf(",");
      resolve(comma >= 0 ? result.slice(comma + 1) : result);
    };
    reader.onerror = () => reject(reader.error ?? new Error("read failed"));
    reader.readAsDataURL(file);
  });
}

export function WorkspaceImportSection({
  workspaceId,
  canManage,
}: {
  workspaceId: string;
  canManage: boolean;
}) {
  const inputRef = useRef<HTMLInputElement>(null);
  const [source, setSource] = useState<ImportSource>("markdown-zip");
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const importMutation = useMutation({
    mutationFn: async (file: File) => {
      const zipBase64 = await fileToBase64(file);
      const response = await fetch("/api/v1/import", {
        method: "POST",
        credentials: "include",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          workspaceId,
          source,
          zipBase64,
          fileName: file.name,
        }),
      });
      if (!response.ok) {
        throw new ProblemError(response.status, "import_failed");
      }
      const job = (await response.json()) as {
        id: string;
        status: ImportJobStatus;
      };
      if (isImportActive(job.status)) {
        const final = await pollImportJob(workspaceId, job.id);
        if (final === "failed") {
          throw new ProblemError(400, "import_failed");
        }
      }
      return job;
    },
    onSuccess: () => {
      setMessage(t("workspace.import.ok"));
      setError(null);
    },
    onError: (err) => {
      setMessage(null);
      setError(err instanceof ProblemError ? err.title : t("workspace.import.failed"));
    },
  });

  if (!canManage) return null;

  return (
    <section className="settings-section">
      <h2 className="settings-section__title text-title">{t("workspace.import.source")}</h2>
      <div className="settings-section__stack">
        <Label htmlFor="import-source">{t("workspace.import.source")}</Label>
        <select
          id="import-source"
          className="settings-input"
          value={source}
          onChange={(event) => setSource(event.target.value as ImportSource)}
        >
          <option value="markdown-zip">{t("workspace.import.source.markdown-zip")}</option>
          <option value="office-file">{t("workspace.import.source.office-file")}</option>
          <option value="notion-zip">{t("workspace.import.source.notion-zip")}</option>
        </select>
        <input
          ref={inputRef}
          type="file"
          accept={IMPORT_ACCEPT[source]}
          className="hidden"
          onChange={(event) => {
            const file = event.target.files?.[0];
            event.target.value = "";
            if (!file) return;
            importMutation.mutate(file);
          }}
        />
        <Button
          type="button"
          disabled={importMutation.isPending}
          onClick={() => inputRef.current?.click()}
        >
          {importMutation.isPending ? t("workspace.import.running") : t("workspace.import.source")}
        </Button>
        {message ? <p className="text-ui text-muted-foreground">{message}</p> : null}
        {error ? <p className="text-ui text-destructive">{error}</p> : null}
      </div>
    </section>
  );
}
