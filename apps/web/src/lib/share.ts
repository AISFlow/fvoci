export function shareDownloadName(title: string, ext: string): string {
  const base = title.trim().replace(/[/\\?%*:|"<>]/g, "_");
  const clipped = base.slice(0, 80);
  return `${clipped.length > 0 ? clipped : "export"}.${ext}`;
}

export function triggerDownload(filename: string, blob: Blob): void {
  const href = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = href;
  link.download = filename;
  link.rel = "noopener";
  document.body.appendChild(link);
  link.click();
  link.remove();
  URL.revokeObjectURL(href);
}

async function downloadWikiBinary(
  workspaceId: string,
  documentId: string,
  title: string,
  ext: "md" | "pdf" | "docx" | "pptx",
  projectId?: string | null,
): Promise<void> {
  const base = projectId
    ? `/api/v1/workspaces/${workspaceId}/projects/${projectId}/documents/${documentId}`
    : `/api/v1/workspaces/${workspaceId}/documents/${documentId}`;
  const response = await fetch(`${base}/${ext}`, { credentials: "include" });
  if (!response.ok) {
    throw new Error(`export failed: ${response.status}`);
  }
  const blob = await response.blob();
  triggerDownload(shareDownloadName(title, ext), blob);
}

export async function downloadDocumentMarkdown(
  workspaceId: string,
  documentId: string,
  title: string,
  projectId?: string | null,
): Promise<void> {
  await downloadWikiBinary(workspaceId, documentId, title, "md", projectId);
}

export async function downloadDocumentPdf(
  workspaceId: string,
  documentId: string,
  title: string,
  projectId?: string | null,
): Promise<void> {
  await downloadWikiBinary(workspaceId, documentId, title, "pdf", projectId);
}

export async function downloadDocumentDocx(
  workspaceId: string,
  documentId: string,
  title: string,
  projectId?: string | null,
): Promise<void> {
  await downloadWikiBinary(workspaceId, documentId, title, "docx", projectId);
}

export async function downloadDocumentPptx(
  workspaceId: string,
  documentId: string,
  title: string,
  projectId?: string | null,
): Promise<void> {
  await downloadWikiBinary(workspaceId, documentId, title, "pptx", projectId);
}
