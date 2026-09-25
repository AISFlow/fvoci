import type {
  AttachmentBlockBridge,
  AttachmentUploadResult,
} from "@fvoci/editor/react";
import type { components } from "@/generated/api";
import { api, ensureOk, ProblemError } from "@/lib/api";

const PARALLEL = 3;
const PART_RETRIES = 2;
/** Total time a part may wait out 503 `Retry-After` (server upload capacity). */
const CAPACITY_WAIT_BUDGET_MS = 120_000;

type CreateAttachmentUploadResponse = components["schemas"]["CreateAttachmentUploadResponse"];
type AttachmentOutput = components["schemas"]["AttachmentOutput"];
type PartTargetRef = CreateAttachmentUploadResponse["parts"][number];

interface UploadPipelineDeps {
  fetchImpl?: (...args: Parameters<typeof fetch>) => ReturnType<typeof fetch>;
  delay?: (ms: number) => Promise<void>;
}

const defaultDelay = (ms: number): Promise<void> =>
  new Promise((resolve) => setTimeout(resolve, ms));

async function abortableDelay(
  ms: number,
  signal?: AbortSignal,
  delay: (ms: number) => Promise<void> = defaultDelay,
): Promise<void> {
  if (signal?.aborted) {
    throw signal.reason instanceof Error ? signal.reason : new Error("Aborted");
  }
  if (!signal) {
    await delay(ms);
    return;
  }
  await new Promise<void>((resolve, reject) => {
    const timer = setTimeout(() => {
      signal.removeEventListener("abort", onAbort);
      resolve();
    }, ms);
    const onAbort = (): void => {
      clearTimeout(timer);
      signal.removeEventListener("abort", onAbort);
      reject(signal.reason instanceof Error ? signal.reason : new Error("Aborted"));
    };
    signal.addEventListener("abort", onAbort);
  });
}

function partChunk(file: File, partNumber: number, partSize: number): Blob {
  return file.slice((partNumber - 1) * partSize, partNumber * partSize);
}

function isAbortError(err: unknown): boolean {
  return err instanceof Error && err.name === "AbortError";
}

function isPermanentPartStatus(status: number): boolean {
  return status === 400 || status === 401 || status === 403 || status === 404 || status === 409 || status === 413;
}

function isPermanentAuthStatus(status: number): boolean {
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

function isPermanentUploadError(err: unknown): boolean {
  if (err instanceof PartUploadError) return isPermanentPartStatus(err.status);
  if (err instanceof ProblemError) return isPermanentPartStatus(err.status);
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

function retryAfterMillis(res: Response): number | null {
  const header = res.headers.get("retry-after");
  if (header === null) return null;
  const seconds = Number(header);
  if (!Number.isFinite(seconds) || seconds < 0) return null;
  return Math.min(seconds, 30) * 1000;
}

async function putPart(
  target: PartTargetRef,
  file: File,
  partSizeBytes: number,
  deps: Required<UploadPipelineDeps>,
  signal?: AbortSignal,
): Promise<{ partNumber: number; etag: string }> {
  let lastError: unknown;
  let capacityWaitMs = 0;
  let retryAfterMs: number | null = null;
  for (let attempt = 0; attempt <= PART_RETRIES; attempt += 1) {
    if (signal?.aborted) {
      throw signal.reason instanceof Error ? signal.reason : new Error("Aborted");
    }
    if (retryAfterMs !== null) {
      // Capacity refusals are answered before the body is read; waiting them
      // out does not use up the transport retries.
      await abortableDelay(retryAfterMs, signal, deps.delay);
      retryAfterMs = null;
    } else if (attempt > 0) {
      await abortableDelay(300 * 2 ** (attempt - 1), signal, deps.delay);
    }
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
      const wait = res.status === 503 ? retryAfterMillis(res) : null;
      if (wait !== null && capacityWaitMs + wait <= CAPACITY_WAIT_BUDGET_MS) {
        capacityWaitMs += wait;
        retryAfterMs = wait;
        attempt -= 1;
        continue;
      }
      if (res.status < 500) break;
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
  if (signal?.aborted) {
    throw signal.reason instanceof Error ? signal.reason : new Error("Aborted");
  }
  try {
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
    if (isPermanentAuthStatus(status)) {
      throw new ProblemError(status, result.error?.code);
    }
    const stored = await storedAttachmentMeta(workspaceId, attachmentId, signal);
    if (stored) return attachmentResult(stored);
    if (isPermanentPartStatus(status)) {
      throw new ProblemError(status, result.error?.code);
    }
    return ensureOk(result);
  } catch (err) {
    if (isAbortError(err) || signal?.aborted) throw err;
    if (err instanceof ProblemError && isPermanentAuthStatus(err.status)) throw err;
    const stored = await storedAttachmentMeta(workspaceId, attachmentId, signal);
    if (stored) return attachmentResult(stored);
    throw err;
  }
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
        if (isPermanentUploadError(err)) throw err;
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
