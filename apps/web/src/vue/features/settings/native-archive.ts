/** One captured workspace/actor/session owns each asynchronous archive flow. */
export class ArchiveLifetime {
  private generation = 0;
  private controller = new AbortController();

  reset(): void {
    this.controller.abort();
    this.controller = new AbortController();
    this.generation++;
  }

  capture(workspaceId: string, actorId: string, sessionId: string) {
    const generation = this.generation;
    const signal = this.controller.signal;
    return {
      workspaceId,
      actorId,
      sessionId,
      signal,
      current: () => generation === this.generation && !signal.aborted,
    };
  }
}

export const MAX_NATIVE_ARCHIVE_BYTES = 64 * 1024 * 1024;

export function readNativeArchive(file: File, signal: AbortSignal): Promise<string> {
  if (file.size > MAX_NATIVE_ARCHIVE_BYTES || file.size === 0) {
    return Promise.reject(new Error("native archive size"));
  }
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    const stop = () => {
      reader.abort();
    };
    const cleanup = () => {
      signal.removeEventListener("abort", stop);
    };
    reader.onload = () => {
      cleanup();
      if (signal.aborted) {
        reject(new DOMException("Cancelled", "AbortError"));
        return;
      }
      if (typeof reader.result !== "string") {
        reject(new Error("archive read"));
        return;
      }
      resolve(reader.result.slice(reader.result.indexOf(",") + 1));
    };
    reader.onerror = () => {
      cleanup();
      reject(reader.error ?? new Error("archive read"));
    };
    reader.onabort = () => {
      cleanup();
      reject(new DOMException("Cancelled", "AbortError"));
    };
    if (signal.aborted) {
      reject(new DOMException("Cancelled", "AbortError"));
      return;
    }
    signal.addEventListener("abort", stop, { once: true });
    reader.readAsDataURL(file);
  });
}

/** A complete preflight is bound to the exact captured destination and bytes. */
export function canConfirmNativeArchive(
  preflight: {
    complete: boolean;
    archiveHash: string;
    destinationWorkspaceId: string;
    destinationActorId: string;
    preservedContentIds: boolean;
    requiresCollisionFreeInstallation: boolean;
    diagnostics: string[];
  },
  workspaceId: string,
  actorId: string,
): boolean {
  return (
    preflight.complete &&
    /^[0-9a-f]{64}$/.test(preflight.archiveHash) &&
    preflight.destinationWorkspaceId === workspaceId &&
    preflight.destinationActorId === actorId &&
    preflight.preservedContentIds &&
    preflight.requiresCollisionFreeInstallation &&
    preflight.diagnostics.length === 0
  );
}
