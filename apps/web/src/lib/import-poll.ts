const IMPORT_POLL_MS = 1500;
const IMPORT_POLL_TRIES = 40;

export type ImportJobStatus = "pending" | "running" | "completed" | "failed";

export function isImportActive(status: ImportJobStatus): boolean {
  return status === "pending" || status === "running";
}

export async function pollImportJob(
  workspaceId: string,
  importJobId: string,
): Promise<ImportJobStatus> {
  for (let i = 0; i < IMPORT_POLL_TRIES; i += 1) {
    const response = await fetch(
      `/api/v1/import/${importJobId}?workspaceId=${encodeURIComponent(workspaceId)}`,
      { credentials: "include" },
    );
    if (!response.ok) {
      throw new Error(`import status failed: ${response.status}`);
    }
    const data = (await response.json()) as { status?: ImportJobStatus };
    const status = data.status;
    if (!status) {
      throw new Error("import status missing");
    }
    if (!isImportActive(status)) {
      return status;
    }
    await new Promise((resolve) => {
      window.setTimeout(resolve, IMPORT_POLL_MS);
    });
  }
  throw new Error("import timeout");
}
