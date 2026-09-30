import type {
  AttachmentBlockBridge,
  AttachmentUploadResult,
} from "@fvoci/editor/attachment-model";
import type { components } from "@/generated/api";
import { api, ensureOk, ProblemError } from "@/lib/api";

// Framework-neutral upload pipeline (React today, Vue later): no UI state.
//
// A session is bound to its transfer mode by the server (#149) and never
// switches: `proxy` PUTs each part to the API with the session cookie;
// `presigned` PUTs it straight to storage with the signed URL alone (no
// cookies, no Authorization, no Content-Type) and reads the ETag the bucket
// CORS exposes. A presigned failure is never retried through the API.

const PARALLEL = 3;
const PART_RETRIES = 2;
/** Re-sends of an idempotent complete after a gateway failure or lost response. */
const COMPLETE_RETRIES = 2;
/** Total time a part may wait out 503 `Retry-After` (server upload capacity). */
const CAPACITY_WAIT_BUDGET_MS = 120_000;
/** Presigned URLs this close to expiry are re-issued before a part is sent. */
const URL_EXPIRY_MARGIN_MS = 60_000;

type CreateAttachmentUploadResponse = components["schemas"]["CreateAttachmentUploadResponse"];
type AttachmentOutput = components["schemas"]["AttachmentOutput"];
type PartTargetRef = CreateAttachmentUploadResponse["parts"][number];
/** Part targets of a create or resume answer, in the session's own mode. */
type PartTargets = Pick<
  CreateAttachmentUploadResponse,
  "transfer" | "partUrlsExpireAt" | "partSizeBytes" | "parts"
>;

interface UploadPipelineDeps {
  fetchImpl?: (...args: Parameters<typeof fetch>) => ReturnType<typeof fetch>;
  delay?: (ms: number) => Promise<void>;
  now?: () => number;
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

/**
 * An untyped part body: a typed Blob would make fetch add a Content-Type
 * header, which the bucket's CORS rule (AllowedHeaders: range) refuses.
 */
function partChunk(file: File, partNumber: number, partSize: number): Blob {
  return file.slice((partNumber - 1) * partSize, partNumber * partSize, "");
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

/**
 * The presigned URLs need re-issuing: they are about to expire (`status`
 * null), or storage refused one with 403. A 403 is usually expiry but may be
 * a signature storage does not accept (a proxy rewrote `Host`, a wrong key),
 * so the message names the refusal rather than guessing.
 */
class PartUrlsExpiredError extends Error {
  constructor(partNumber: number, status: number | null) {
    super(
      status === null
        ? `part ${partNumber}: presigned URL about to expire`
        : `part ${partNumber}: storage refused the signed URL (HTTP ${status})`,
    );
    this.name = "PartUrlsExpiredError";
  }
}

/** Storage accepted the part but the bucket CORS hides its ETag: resending cannot help. */
class MissingEtagError extends Error {
  constructor(partNumber: number) {
    super(`part ${partNumber}: storage did not expose the ETag header (bucket CORS ExposeHeaders)`);
    this.name = "MissingEtagError";
  }
}

function isPermanentUploadError(err: unknown): boolean {
  if (err instanceof MissingEtagError) return true;
  if (err instanceof PartUploadError) return isPermanentPartStatus(err.status);
  if (err instanceof ProblemError) return isPermanentPartStatus(err.status);
  return false;
}

/**
 * Client-clock time after which the presigned targets are re-issued before
 * sending a part, or null (proxy targets, or a lifetime that does not fit the
 * client clock, where only storage's own 403 on an expired URL counts).
 */
function refreshAt(targets: PartTargets, receivedAt: number): number | null {
  if (targets.transfer !== "presigned" || !targets.partUrlsExpireAt) return null;
  const expiresAt = Date.parse(targets.partUrlsExpireAt);
  if (!Number.isFinite(expiresAt) || expiresAt - receivedAt < 2 * URL_EXPIRY_MARGIN_MS) return null;
  return expiresAt - URL_EXPIRY_MARGIN_MS;
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
  // At least 1 s so a zero value still spends the capacity budget.
  return Math.max(1, Math.min(seconds, 30)) * 1000;
}

/**
 * One part straight to storage. The signed URL is the whole authorization:
 * `credentials: "omit"` keeps FVOCI cookies away from the storage origin, and
 * no header is set, because the URL signs only `host` and the exact
 * `content-length` the browser derives from the Blob. 403 means the URL
 * expired (or storage refused its signature); it is re-issued, never retried
 * through the API.
 */
async function putPresignedPart(
  target: PartTargetRef,
  file: File,
  partSizeBytes: number,
  deps: Required<UploadPipelineDeps>,
  refreshAfter: number | null,
  signal?: AbortSignal,
): Promise<{ partNumber: number; etag: string }> {
  let lastError: unknown;
  for (let attempt = 0; attempt <= PART_RETRIES; attempt += 1) {
    if (signal?.aborted) {
      throw signal.reason instanceof Error ? signal.reason : new Error("Aborted");
    }
    if (attempt > 0) await abortableDelay(300 * 2 ** (attempt - 1), signal, deps.delay);
    if (refreshAfter !== null && deps.now() >= refreshAfter) {
      throw new PartUrlsExpiredError(target.partNumber, null);
    }
    try {
      const res = await deps.fetchImpl(target.url, {
        method: "PUT",
        credentials: "omit",
        body: partChunk(file, target.partNumber, partSizeBytes),
        signal,
      });
      if (res.ok) {
        const etag = res.headers.get("etag");
        if (!etag) throw new MissingEtagError(target.partNumber);
        return { partNumber: target.partNumber, etag };
      }
      if (res.status === 403) throw new PartUrlsExpiredError(target.partNumber, res.status);
      lastError = new PartUploadError(target.partNumber, res.status);
      if (res.status < 500) break;
    } catch (err) {
      if (isAbortError(err) || signal?.aborted) throw err;
      if (err instanceof PartUrlsExpiredError || err instanceof MissingEtagError) throw err;
      // A network or CORS failure: bounded retries, then one resume.
      lastError = err;
    }
  }
  throw lastError instanceof Error
    ? lastError
    : new Error(`part ${target.partNumber} failed`);
}

async function putProxyPart(
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
  targets: PartTargets,
  receivedAt: number,
  file: File,
  onPartDone: () => void,
  deps: Required<UploadPipelineDeps>,
  signal?: AbortSignal,
): Promise<{ partNumber: number; etag: string }[]> {
  const refreshAfter = refreshAt(targets, receivedAt);
  const put = (target: PartTargetRef) =>
    targets.transfer === "presigned"
      ? putPresignedPart(target, file, targets.partSizeBytes, deps, refreshAfter, signal)
      : putProxyPart(target, file, targets.partSizeBytes, deps, signal);
  const queue = [...targets.parts];
  const done: { partNumber: number; etag: string }[] = [];
  let aborted = false;
  const workers = Array.from({ length: Math.min(PARALLEL, queue.length) }, async () => {
    for (;;) {
      if (aborted) return;
      const next = queue.shift();
      if (!next) return;
      try {
        done.push(await put(next));
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

/**
 * Gateway failures where the complete may not have run, may still be running,
 * or may have finished with its response lost (e.g. a reverse proxy's 502/504
 * or Cloudflare's 520–524). Complete is idempotent on the server, so these are
 * retried a bounded number of times after checking for a stored attachment.
 */
function isTransientCompleteStatus(status: number): boolean {
  return status === 502 || status === 503 || status === 504 || (status >= 520 && status <= 524);
}

type CompleteAttempt = { done: AttachmentUploadResult } | { retry: unknown };

/** Stored metadata, or null when it is not stored yet or cannot be read now. */
async function reconcileStored(
  workspaceId: string,
  attachmentId: string,
  signal?: AbortSignal,
): Promise<AttachmentOutput | null> {
  try {
    return await storedAttachmentMeta(workspaceId, attachmentId, signal);
  } catch (err) {
    if (isAbortError(err) || signal?.aborted) throw err;
    return null;
  }
}

async function completeOnce(
  workspaceId: string,
  attachmentId: string,
  parts: { partNumber: number; etag: string }[],
  signal?: AbortSignal,
): Promise<CompleteAttempt> {
  let result;
  try {
    result = await api.POST(
      "/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/complete",
      {
        params: {
          path: { workspace_id: workspaceId, attachment_id: attachmentId },
        },
        body: { parts },
        signal,
      },
    );
  } catch (err) {
    if (isAbortError(err) || signal?.aborted) throw err;
    // The request never got a readable HTTP answer (connection reset, proxy drop).
    const stored = await reconcileStored(workspaceId, attachmentId, signal);
    if (stored) return { done: attachmentResult(stored) };
    return { retry: err };
  }
  if (result.response.ok && result.data) return { done: attachmentResult(result.data) };
  // The complete's own answer decides what is thrown or retried; a metadata
  // lookup only turns it into success when the attachment is already stored.
  const status = result.response.status;
  const problem = result.response.ok
    ? new ProblemError(500)
    : new ProblemError(status, result.error?.code);
  if (isPermanentAuthStatus(status)) throw problem;
  const stored = await reconcileStored(workspaceId, attachmentId, signal);
  if (stored) return { done: attachmentResult(stored) };
  if (isTransientCompleteStatus(status)) return { retry: problem };
  throw problem;
}

async function completeUpload(
  workspaceId: string,
  attachmentId: string,
  parts: { partNumber: number; etag: string }[],
  deps: Required<UploadPipelineDeps>,
  signal?: AbortSignal,
): Promise<AttachmentUploadResult> {
  let lastError: unknown;
  for (let attempt = 0; attempt <= COMPLETE_RETRIES; attempt += 1) {
    if (attempt > 0) {
      await abortableDelay(1000 * 2 ** (attempt - 1), signal, deps.delay);
    }
    if (signal?.aborted) {
      throw signal.reason instanceof Error ? signal.reason : new Error("Aborted");
    }
    const outcome = await completeOnce(workspaceId, attachmentId, parts, signal);
    if ("done" in outcome) return outcome.done;
    lastError = outcome.retry;
  }
  throw lastError instanceof Error ? lastError : new Error("complete failed");
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
    now: pipelineDeps.now ?? Date.now,
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
      let progressed = false;
      const tick = (): void => {
        finished += 1;
        progressed = true;
        onProgress(total === 0 ? 1 : finished / total);
      };
      let targets: PartTargets = created;
      let receivedAt = deps.now();
      let uploaded: { partNumber: number; etag: string }[] = [];
      // One resume after a transfer failure, as before #149. Expired presigned
      // URLs are re-issued as often as parts keep completing in between, so a
      // long upload outlives the URL lifetime but a URL storage keeps refusing
      // cannot loop.
      let failureResumed = false;
      let expiredWithoutProgress = 0;
      let parts: { partNumber: number; etag: string }[];
      for (;;) {
        progressed = false;
        try {
          const rest = await putParts(targets, receivedAt, file, tick, deps, signal);
          parts = [...uploaded, ...rest];
          break;
        } catch (err) {
          if (isAbortError(err) || signal?.aborted) throw err;
          if (err instanceof PartUrlsExpiredError) {
            expiredWithoutProgress = progressed ? 1 : expiredWithoutProgress + 1;
            if (expiredWithoutProgress > 1) throw err;
          } else {
            if (isPermanentUploadError(err) || failureResumed) throw err;
            failureResumed = true;
          }
        }
        const resumed = await ensureOk(
          await api.GET("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/upload", {
            params: {
              path: { workspace_id: workspaceId, attachment_id: created.attachmentId },
            },
            signal,
          }),
        );
        receivedAt = deps.now();
        targets = resumed;
        uploaded = resumed.uploadedParts;
        finished = uploaded.length;
        onProgress(total === 0 ? 1 : finished / total);
      }
      return await completeUpload(workspaceId, created.attachmentId, parts, deps, signal);
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

function uploadBody(file: File): components["schemas"]["CreateAttachmentUploadBody"] {
  return {
    name: file.name,
    sizeBytes: file.size,
    ...(file.type ? { declaredMime: file.type } : {}),
  };
}

export function createProjectDocumentAttachmentBridge(
  workspaceId: string,
  projectId: string,
  documentId: string,
  pipelineDeps: UploadPipelineDeps = {},
): AttachmentBlockBridge {
  return uploadsBridge(
    workspaceId,
    async (file, signal) => {
      const result = await api.POST(
        "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/uploads",
        {
          params: {
            path: { workspace_id: workspaceId, project_id: projectId, document_id: documentId },
          },
          body: uploadBody(file),
          signal,
        },
      );
      if (result.response.status === 201 && result.data) return result.data;
      return ensureOk(result);
    },
    pipelineDeps,
  );
}

export function createTaskAttachmentBridge(
  workspaceId: string,
  taskId: string,
  pipelineDeps: UploadPipelineDeps = {},
): AttachmentBlockBridge {
  return uploadsBridge(
    workspaceId,
    async (file, signal) => {
      const result = await api.POST("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/uploads", {
        params: { path: { workspace_id: workspaceId, task_id: taskId } },
        body: uploadBody(file),
        signal,
      });
      if (result.response.status === 201 && result.data) return result.data;
      return ensureOk(result);
    },
    pipelineDeps,
  );
}

/** Source `editedCopyName`: `name (edited).ext`. */
export function editedCopyName(originalName: string): string {
  const dot = originalName.lastIndexOf(".");
  if (dot <= 0) return `${originalName} (edited)`;
  return `${originalName.slice(0, dot)} (edited)${originalName.slice(dot)}`;
}

/** Source `createEditedAttachmentBridge`: saves an edited HWP/HWPX copy beside the source. */
export function createEditedAttachmentBridge(
  workspaceId: string,
  sourceAttachmentId: string,
  pipelineDeps: UploadPipelineDeps = {},
): AttachmentBlockBridge {
  return uploadsBridge(
    workspaceId,
    async (file, signal) => {
      const result = await api.POST(
        "/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/edit-copy",
        {
          params: { path: { workspace_id: workspaceId, attachment_id: sourceAttachmentId } },
          body: uploadBody(file),
          signal,
        },
      );
      if (result.response.status === 201 && result.data) return result.data;
      return ensureOk(result);
    },
    pipelineDeps,
  );
}
