import type {
  AttachmentBlockBridge,
  AttachmentUploadResult,
} from "@fvoci/editor/react";
import type { components } from "@/generated/api";
import { api, ensureOk, ProblemError } from "@/lib/api";

const PARALLEL = 3;
const PART_RETRIES = 2;

type CreateAttachmentUploadResponse = components["schemas"]["CreateAttachmentUploadResponse"];
type AttachmentOutput = components["schemas"]["AttachmentOutput"];

interface PartTargetRef {
  partNumber: number;
  url: string;
}

interface UploadPipelineDeps {
  fetchImpl?: (...args: Parameters<typeof fetch>) => ReturnType<typeof fetch>;
  delay?: (ms: number) => Promise<void>;
}

const defaultDelay = (ms: number): Promise<void> =>
  new Promise((resolve) => setTimeout(resolve, ms));

function partChunk(file: File, partNumber: number, partSize: number): Blob {
  return file.slice((partNumber - 1) * partSize, partNumber * partSize);
}

function isAbortError(err: unknown): boolean {
  return err instanceof Error && err.name === "AbortError";
}

function isNonRetryablePartStatus(status: number): boolean {
  return status === 401 || status === 403 || status === 404;
}

class PartUploadError extends Error {
  readonly status: number;

  constructor(partNumber: number, status: number) {
    super(`part ${partNumber}: ${status}`);
    this.name = "PartUploadError";
    this.status = status;
  }
}

function isNonRetryableUploadError(err: unknown): boolean {
  if (err instanceof PartUploadError) return isNonRetryablePartStatus(err.status);
  if (err instanceof ProblemError) return isNonRetryablePartStatus(err.status);
  return false;
}

async function readPartEtag(res: Response): Promise<string> {
  const header = res.headers.get("etag");
  if (header) return header;
  const body: unknown = await res.json();
  if (
    typeof body === "object" &&
    body !== null &&
    "etag" in body &&
    typeof body.etag === "string"
  ) {
    return body.etag;
  }
  throw new Error("missing etag");
}

async function putPart(
  target: PartTargetRef,
  file: File,
  partSizeBytes: number,
  deps: Required<UploadPipelineDeps>,
  signal?: AbortSignal,
): Promise<{ partNumber: number; etag: string }> {
  let lastError: unknown;
  for (let attempt = 0; attempt <= PART_RETRIES; attempt += 1) {
    if (signal?.aborted) {
      throw signal.reason instanceof Error ? signal.reason : new Error("Aborted");
    }
    if (attempt > 0) await deps.delay(300 * 2 ** (attempt - 1));
    try {
      const res = await deps.fetchImpl(target.url, {
        method: "PUT",
        credentials: "include",
        headers: { "Content-Type": "application/octet-stream" },
        body: partChunk(file, target.partNumber, partSizeBytes),
        signal,
      });
      if (res.ok) {
        const etag = await readPartEtag(res);
        return { partNumber: target.partNumber, etag };
      }
      lastError = new PartUploadError(target.partNumber, res.status);
      if (res.status < 500 || isNonRetryablePartStatus(res.status)) break;
    } catch (err) {
      if (isAbortError(err) || signal?.aborted) throw err;
      lastError = err;
    }
  }
  throw lastError instanceof Error
    ? lastError
    : new Error(`part ${target.partNumber} failed`);
}

async function putParts(
  targets: PartTargetRef[],
  file: File,
  partSizeBytes: number,
  onPartDone: () => void,
  deps: Required<UploadPipelineDeps>,
  signal?: AbortSignal,
): Promise<{ partNumber: number; etag: string }[]> {
  const queue = [...targets];
  const done: { partNumber: number; etag: string }[] = [];
  let aborted = false;
  const workers = Array.from({ length: Math.min(PARALLEL, queue.length) }, async () => {
    for (;;) {
      if (aborted) return;
      const next = queue.shift();
      if (!next) return;
      try {
        done.push(await putPart(next, file, partSizeBytes, deps, signal));
      } catch (err) {
        aborted = true;
        throw err;
      }
      onPartDone();
    }
  });
  const settled = await Promise.allSettled(workers);
  const rejected = settled.filter((s): s is PromiseRejectedResult => s.status === "rejected");
  if (rejected[0]) throw rejected[0].reason;
  return done;
}

async function storedAttachmentMeta(
  workspaceId: string,
  attachmentId: string,
  signal?: AbortSignal,
): Promise<AttachmentOutput | null> {
  const result = await api.GET("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}", {
    params: {
      path: { workspace_id: workspaceId, attachment_id: attachmentId },
    },
    signal,
  });
  if (result.response.ok && result.data?.completedAt) return result.data;
  return null;
}

async function completeUpload(
  workspaceId: string,
  attachmentId: string,
  parts: { partNumber: number; etag: string }[],
  signal?: AbortSignal,
): Promise<AttachmentUploadResult> {
  const result = await api.POST(
    "/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/complete",
    {
      params: {
        path: { workspace_id: workspaceId, attachment_id: attachmentId },
      },
      body: { parts },
      signal,
    },
  );
  if (result.response.ok && result.data) return attachmentResult(result.data);
  const status = result.response.status;
  if (isNonRetryablePartStatus(status)) {
    throw new ProblemError(status, result.error?.code);
  }
  const stored = await storedAttachmentMeta(workspaceId, attachmentId, signal);
  if (stored) return attachmentResult(stored);
  return ensureOk(result);
}

function attachmentResult(att: AttachmentOutput): AttachmentUploadResult {
  return {
    id: att.id,
    name: att.name,
    image: att.image,
  };
}

function uploadsBridge(
  workspaceId: string,
  createUpload: (file: File, signal?: AbortSignal) => Promise<CreateAttachmentUploadResponse>,
  pipelineDeps: UploadPipelineDeps,
): AttachmentBlockBridge {
  const deps: Required<UploadPipelineDeps> = {
    fetchImpl: pipelineDeps.fetchImpl ?? fetch.bind(globalThis),
    delay: pipelineDeps.delay ?? defaultDelay,
  };
  return {
    async attachmentMeta(attachmentId) {
      const att = await ensureOk(
        await api.GET("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}", {
          params: {
            path: { workspace_id: workspaceId, attachment_id: attachmentId },
          },
        }),
      );
      return { sizeBytes: att.sizeBytes, mime: att.mime, preview: att.preview };
    },
    async upload(file, onProgress, signal) {
      const created = await createUpload(file, signal);
      const total = created.parts.length;
      let finished = 0;
      const tick = (): void => {
        finished += 1;
        onProgress(total === 0 ? 1 : finished / total);
      };
      let parts: { partNumber: number; etag: string }[];
      try {
        parts = await putParts(
          created.parts,
          file,
          created.partSizeBytes,
          tick,
          deps,
          signal,
        );
      } catch (err) {
        if (isAbortError(err) || signal?.aborted) throw err;
        if (isNonRetryableUploadError(err)) throw err;
        const resumed = await ensureOk(
          await api.GET("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/upload", {
            params: {
              path: { workspace_id: workspaceId, attachment_id: created.attachmentId },
            },
            signal,
          }),
        );
        finished = resumed.uploadedParts.length;
        onProgress(total === 0 ? 1 : finished / total);
        const rest = await putParts(
          resumed.parts,
          file,
          resumed.partSizeBytes,
          tick,
          deps,
          signal,
        );
        parts = [...resumed.uploadedParts, ...rest];
      }
      return await completeUpload(workspaceId, created.attachmentId, parts, signal);
    },
    downloadUrl(attachmentId) {
      return `/api/v1/workspaces/${workspaceId}/attachments/${attachmentId}/download`;
    },
  };
}

async function createDocumentUpload(
  workspaceId: string,
  documentId: string,
  file: File,
  signal?: AbortSignal,
): Promise<CreateAttachmentUploadResponse> {
  const body: components["schemas"]["CreateAttachmentUploadBody"] = {
    name: file.name,
    sizeBytes: file.size,
    ...(file.type ? { declaredMime: file.type } : {}),
  };
  const result = await api.POST(
    "/api/v1/workspaces/{workspace_id}/documents/{document_id}/uploads",
    {
      params: {
        path: { workspace_id: workspaceId, document_id: documentId },
      },
      body,
      signal,
    },
  );
  if (result.response.status === 201 && result.data) return result.data;
  return ensureOk(result);
}

export function createAttachmentBridge(
  workspaceId: string,
  documentId: string,
  pipelineDeps: UploadPipelineDeps = {},
): AttachmentBlockBridge {
  return uploadsBridge(
    workspaceId,
    (file, signal) => createDocumentUpload(workspaceId, documentId, file, signal),
    pipelineDeps,
  );
}
