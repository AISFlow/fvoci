import { t } from "@fvoci/i18n";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import {
  downloadDocumentDocx,
  downloadDocumentMarkdown,
  downloadDocumentPdf,
  downloadDocumentPptx,
} from "@/lib/share";

export function DocumentExportMenu({
  workspaceId,
  documentId,
  title,
  projectId = null,
  persistNow,
}: {
  workspaceId: string;
  documentId: string;
  title: string;
  projectId?: string | null;
  persistNow?: () => Promise<void>;
}) {
  const [pending, setPending] = useState(false);

  function runExport(
    work: () => Promise<void>,
    failKey:
      | "export.md.failed"
      | "export.pdf.failed"
      | "export.docx.failed"
      | "export.pptx.failed",
  ) {
    setPending(true);
    void (persistNow?.() ?? Promise.resolve())
      .then(work)
      .catch(() => {
        window.alert(t(failKey));
      })
      .finally(() => {
        setPending(false);
      });
  }

  return (
    <div className="document-export-menu flex flex-wrap gap-2">
      <Button
        type="button"
        variant="outline"
        size="sm"
        disabled={pending}
        onClick={() =>
          runExport(
            () => downloadDocumentMarkdown(workspaceId, documentId, title, projectId),
            "export.md.failed",
          )
        }
      >
        {t("export.md")}
      </Button>
      <Button
        type="button"
        variant="outline"
        size="sm"
        disabled={pending}
        onClick={() =>
          runExport(
            () => downloadDocumentPdf(workspaceId, documentId, title, projectId),
            "export.pdf.failed",
          )
        }
      >
        {t("export.pdf")}
      </Button>
      <Button
        type="button"
        variant="outline"
        size="sm"
        disabled={pending}
        onClick={() =>
          runExport(
            () => downloadDocumentDocx(workspaceId, documentId, title, projectId),
            "export.docx.failed",
          )
        }
      >
        {t("export.docx")}
      </Button>
      <Button
        type="button"
        variant="outline"
        size="sm"
        disabled={pending}
        onClick={() =>
          runExport(
            () => downloadDocumentPptx(workspaceId, documentId, title, projectId),
            "export.pptx.failed",
          )
        }
      >
        {t("export.pptx")}
      </Button>
    </div>
  );
}
