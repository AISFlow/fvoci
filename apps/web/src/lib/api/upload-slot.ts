/**
 * Multipart upload and archive import/export helpers with concurrency limiting
 * (`UPLOADS_AT_ONCE = 3`) for `ServerBackend`.
 */
import type {
  DownloadedFile,
  DownloadOptions,
  ImportOptions,
  ImportSummary,
  UploadedFile,
  UploadOptions,
} from '../state/backend.ts';

import type { ClientConfig } from './content-search.ts';
import { downloadWithProgress } from './download.ts';
import type { Http } from './http.ts';
import { underWorkspace } from './requests.ts';
import { importInParts, tooLarge, uploadInParts } from './upload.ts';

const UPLOADS_AT_ONCE = 3;

export class ServerTransfers {
  readonly #http: Http;
  readonly #config: ClientConfig;
  #uploading = 0;
  readonly #waiting: (() => void)[] = [];

  constructor(http: Http, config: ClientConfig) {
    this.#http = http;
    this.#config = config;
  }

  async upload(
    workspaceId: string,
    containerId: string,
    file: File,
    options?: UploadOptions,
  ): Promise<UploadedFile> {
    const limit = (await this.#config.read()).maxFileBytes;
    if (file.size > limit) throw tooLarge('file', limit);
    await this.#turn(options?.signal);
    try {
      return await uploadInParts(this.#http, { workspaceId, containerId }, file, options);
    } finally {
      this.#release();
    }
  }

  async importArchive(
    workspaceId: string,
    file: File,
    options?: ImportOptions,
  ): Promise<ImportSummary> {
    const limit = (await this.#config.read()).maxImportBytes;
    if (file.size > limit) throw tooLarge('archive', limit);
    await this.#turn(options?.signal);
    try {
      return await importInParts(this.#http, workspaceId, file, options);
    } finally {
      this.#release();
    }
  }

  exportContainer(
    params: { workspaceId: string; containerId: string },
    options?: DownloadOptions,
  ): Promise<DownloadedFile> {
    return downloadWithProgress(
      this.#http,
      underWorkspace('/containers/{containerId}/archive/export'),
      params,
      'archive.fvoci',
      options,
    );
  }

  exportWorkspace(workspaceId: string, options?: DownloadOptions): Promise<DownloadedFile> {
    return downloadWithProgress(
      this.#http,
      underWorkspace('/archive/export'),
      { workspaceId },
      'archive.fvoci',
      options,
    );
  }

  async #turn(signal: AbortSignal | undefined): Promise<void> {
    while (this.#uploading >= UPLOADS_AT_ONCE) {
      signal?.throwIfAborted();
      await new Promise<void>((resolve, reject) => {
        const giveUp = (): void => {
          const at = this.#waiting.indexOf(wake);
          if (at >= 0) this.#waiting.splice(at, 1);
          reject(signal?.reason as Error);
        };
        const wake = (): void => {
          signal?.removeEventListener('abort', giveUp);
          resolve();
        };
        this.#waiting.push(wake);
        signal?.addEventListener('abort', giveUp, { once: true });
      });
    }
    signal?.throwIfAborted();
    this.#uploading += 1;
  }

  #release(): void {
    this.#uploading -= 1;
    this.#waiting.shift()?.();
  }
}
