/**
 * The versions of the files of the backend in memory, as the server keeps
 * them: a file that was never written anew has its upload as its only
 * version; from the second on each is kept, the newest `LIMITS.fileVersions`
 * of them. Read by who reads the board or wiki, written by who edits it. The
 * attachments that give a file say the size of its newest version.
 */
import type { UploadedFile } from '../backend.ts';
import {
  FILE_BEING_EDITED,
  FILE_VERSION_STALE,
  type FileLease,
  type FileVersion,
  type FileVersions,
  type HeldFileLease,
} from '../file-versions.ts';

import { memberRole } from './container-access.ts';
import { readableFile } from './files.ts';
import { fail } from './refuse.ts';
import { bodyFileIds, LIMITS, wireOf } from './shapes.ts';
import type { FileLeaseHeld, KeptFileVersion, StoredFile, World } from './world.ts';

const extensionOf = (name: string): string => /\.[^./]+$/.exec(name)?.[0]?.toLowerCase() ?? '';

/** The versions kept of a file, the oldest first: its upload alone until it is written anew. */
function keptOf(file: StoredFile): KeptFileVersion[] {
  return (
    file.versions ?? [
      {
        version: 1,
        createdAt: file.createdAt ?? 0,
        createdBy: file.createdBy ?? '',
        size: file.size,
        restoredFrom: null,
        ...(file.blob ? { blob: file.blob } : {}),
      },
    ]
  );
}

const told = (kept: KeptFileVersion, newest: number): FileVersion => ({
  version: kept.version,
  createdAt: kept.createdAt,
  createdBy: kept.createdBy,
  size: kept.size,
  isCurrent: kept.version === newest,
  restoredFrom: kept.restoredFrom,
});

export class MemoryFileVersions implements FileVersions {
  readonly #backend: { readonly world: World; readonly userId: string };
  readonly #enter: () => Promise<void>;

  /** `backend`: whose world and reader, read at each call. */
  constructor(
    backend: { readonly world: World; readonly userId: string },
    enter: () => Promise<void>,
  ) {
    this.#backend = backend;
    this.#enter = enter;
  }

  get #world(): World {
    return this.#backend.world;
  }

  get #userId(): string {
    return this.#backend.userId;
  }

  async list(containerId: string, fileId: string): Promise<FileVersion[]> {
    await this.#enter();
    const kept = keptOf(readableFile(this.#world, this.#userId, containerId, fileId));
    const newest = kept.at(-1)?.version ?? 1;
    return kept.map((one) => told(one, newest)).reverse();
  }

  async bytes(containerId: string, fileId: string, version: number): Promise<Blob> {
    await this.#enter();
    const kept = keptOf(readableFile(this.#world, this.#userId, containerId, fileId));
    const blob = kept.find((one) => one.version === version)?.blob;
    if (!blob) fail(404, 'no such version');
    return blob;
  }

  async add(
    containerId: string,
    fileId: string,
    baseVersion: number,
    upload: UploadedFile,
  ): Promise<FileVersion> {
    await this.#enter();
    const file = this.#held(containerId, fileId, baseVersion);
    const source = this.#world.files.get(upload.fileId);
    const shown = [...this.#world.blocks.values()].some(
      (block) =>
        block.file?.fileId === upload.fileId ||
        (block.body ? bodyFileIds(block.body).includes(upload.fileId) : false),
    );
    if (
      source?.containerId !== containerId ||
      source === file ||
      source.versions !== undefined ||
      shown ||
      extensionOf(source.name) !== extensionOf(file.name)
    ) {
      fail(400, 'the uploaded file cannot become a version', { field: 'fileId' });
    }
    this.#world.files.delete(source.fileId);
    file.contentType = source.contentType;
    return this.#write(file, source.size, source.blob, null);
  }

  async restore(
    containerId: string,
    fileId: string,
    version: number,
    baseVersion: number,
  ): Promise<FileVersion> {
    await this.#enter();
    const file = this.#held(containerId, fileId, baseVersion);
    const kept = keptOf(file);
    const source = kept.find((one) => one.version === version);
    if (!source) fail(404, 'no such version');
    const newest = kept.at(-1)?.version ?? 1;
    if (source.version === newest) return told(source, newest);
    return this.#write(file, source.size, source.blob, source.version);
  }

  async lease(containerId: string, fileId: string): Promise<FileLease> {
    await this.#enter();
    readableFile(this.#world, this.#userId, containerId, fileId);
    const held = this.#leaseOf(containerId, fileId, this.#world.now());
    return { editing: held !== undefined, editorId: held?.userId ?? '' };
  }

  async holdLease(containerId: string, fileId: string, leaseId: string): Promise<HeldFileLease> {
    await this.#enter();
    this.#editable(containerId, fileId);
    const now = this.#world.now();
    const seconds = LIMITS.fileLeaseSeconds;
    const until = now + seconds * 1000;
    const held = this.#leaseOf(containerId, fileId, now);
    if (held && (held.id !== leaseId || held.userId !== this.#userId)) {
      fail(409, 'somebody else is editing the file', { reason: FILE_BEING_EDITED });
    }
    if (held) {
      held.until = until;
      return { leaseId: held.id, seconds };
    }
    const id = this.#world.newId();
    this.#world.fileLeases.set(`${containerId}/${fileId}`, { id, userId: this.#userId, until });
    return { leaseId: id, seconds };
  }

  async releaseLease(containerId: string, fileId: string, leaseId: string): Promise<void> {
    await this.#enter();
    this.#editable(containerId, fileId);
    const key = `${containerId}/${fileId}`;
    const held = this.#world.fileLeases.get(key);
    if (held?.id === leaseId && held.userId === this.#userId) this.#world.fileLeases.delete(key);
  }

  /** The file's lease, held at `now`; one that ended goes. */
  #leaseOf(containerId: string, fileId: string, now: number): FileLeaseHeld | undefined {
    const key = `${containerId}/${fileId}`;
    const held = this.#world.fileLeases.get(key);
    if (held && held.until <= now) this.#world.fileLeases.delete(key);
    return held && held.until > now ? held : undefined;
  }

  /** The file, for who may write its versions. */
  #editable(containerId: string, fileId: string): StoredFile {
    const role = memberRole(this.#world, this.#userId, containerId, 'any');
    const file = readableFile(this.#world, this.#userId, containerId, fileId);
    if (role !== 'admin' && role !== 'editor') fail(403, 'access denied to make board changes');
    return file;
  }

  /** The file, for who may write its versions from the version `baseVersion`. */
  #held(containerId: string, fileId: string, baseVersion: number): StoredFile {
    const file = this.#editable(containerId, fileId);
    const newest = keptOf(file).at(-1)?.version ?? 1;
    if (newest !== baseVersion) {
      fail(409, `the file's newest version is ${String(newest)}, not ${String(baseVersion)}`, {
        reason: FILE_VERSION_STALE,
      });
    }
    return file;
  }

  #write(
    file: StoredFile,
    size: number,
    blob: Blob | undefined,
    restoredFrom: number | null,
  ): FileVersion {
    const kept = keptOf(file);
    const made: KeptFileVersion = {
      version: (kept.at(-1)?.version ?? 1) + 1,
      createdAt: this.#world.now(),
      createdBy: this.#userId,
      size,
      restoredFrom,
      ...(blob ? { blob } : {}),
    };
    file.versions = [...kept, made].slice(-LIMITS.fileVersions);
    file.size = size;
    if (blob) file.blob = blob;
    else delete file.blob;
    this.#resize(file);
    return told(made, made.version);
  }

  /** The attachments that give the file say its newest size, and who listens is told. */
  #resize(file: StoredFile): void {
    const changed = [...this.#world.blocks.values()].filter(
      (block) => block.file?.fileId === file.fileId && block.file.size !== file.size,
    );
    for (const block of changed) {
      if (block.file) block.file = { ...block.file, size: file.size };
      block.updatedAt = this.#world.now();
      block.modifiedBy = this.#userId;
    }
    if (changed.length > 0) {
      this.#world.tellContainer(file.containerId, {
        kind: 'blocks',
        containerId: file.containerId,
        blocks: changed.map(wireOf),
      });
    }
  }
}
