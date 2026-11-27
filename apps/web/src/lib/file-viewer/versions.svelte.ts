/**
 * The versions of the file open in the viewer, as its list reads them: the
 * list, read when this is made, when it is shown again and after a write;
 * writing a new version from a file the reader picks; and going back to an
 * older one.
 *
 * A version is written from the newest the list had. When the file has a
 * newer one by then (somebody else wrote it meanwhile), nothing is changed:
 * the list is read again, and a file that was uploaded to become a version is
 * kept (`apart`) for the reader to attach on its own instead, or to let go.
 */

import type { LeaseSource } from './lease.svelte.ts';

/** A version kept of a file. */
export interface FileVersion {
  /** Its place among the file's, counted from 1. */
  version: number;
  /** When it was written; for the first, when the file was uploaded. */
  createdAt: number;
  /** Who wrote it; empty where nobody is known to have. */
  createdBy: string;
  /** Its size in bytes. */
  size: number;
  /** It is the file's newest: what the file is read as. */
  isCurrent: boolean;
  /** The version whose bytes it took, for one made by going back; else `null`. */
  restoredFrom: number | null;
}

/** A file that went up to become a version, as the upload answered. */
export interface VersionUpload {
  fileId: string;
  name: string;
  size: number;
  contentType: string;
}

/** Whose versions a list reads, and what the reader may do with them; who edits the file (lease.svelte.ts). */
export interface VersionsSource extends LeaseSource {
  /** Whether the reader may write a version: upload one, go back to one. */
  readonly canWrite: boolean;
  /** The newest first. */
  list(): Promise<FileVersion[]>;
  /** The bytes of a version; undefined when not read. */
  bytes(version: number, signal?: AbortSignal): Promise<Blob | undefined>;
  /**
   * Uploads a file that is to become a version: undefined when it did not go up
   * (told elsewhere), `stopped` when the reader gave it up there.
   */
  upload(file: File): Promise<VersionUpload | 'stopped' | undefined>;
  add(baseVersion: number, upload: VersionUpload): Promise<FileVersion>;
  restore(version: number, baseVersion: number): Promise<FileVersion>;
  /** Attaches an uploaded file where the file is attached, as a file of its own. */
  attachApart(upload: VersionUpload): Promise<void>;
  /** Why a write that threw `error` did not go through. */
  refusalOf(error: unknown): WriteRefusal;
  /** The name to show for who wrote a version; undefined for someone unknown. */
  userName(userId: string): string | undefined;
  /** Reads the names of those the list names. */
  learnUsers(userIds: string[]): Promise<void>;
}

/**
 * Why a write did not go through: the file got a newer version meanwhile
 * (`stale`), the version to go back to is kept no more (`gone`), the file
 * picked cannot be a version of this one (`unusable`), or it failed.
 */
export type WriteRefusal = 'stale' | 'gone' | 'unusable' | 'failed';

export class VersionsList {
  status = $state<'loading' | 'ready' | 'failed'>('loading');
  /** The newest first. */
  versions = $state.raw<FileVersion[]>([]);
  /** A write is under way: the list offers none meanwhile. */
  writing = $state<'upload' | 'restore' | 'apart'>();
  refusal = $state<WriteRefusal>();
  /** A file that went up and did not become a version (`stale`). */
  apart = $state.raw<VersionUpload>();

  /** The first reading of the list, over whether it was read or not. */
  readonly first: Promise<void>;

  readonly #source: VersionsSource;

  constructor(source: VersionsSource) {
    this.#source = source;
    this.first = this.read();
  }

  /** The file's newest version, as the list has it. */
  get current(): FileVersion | undefined {
    return this.versions.find((version) => version.isCurrent);
  }

  /** Reads the list (again). */
  async read(): Promise<void> {
    // A list that is shown stays while it is read again.
    if (this.versions.length === 0) this.status = 'loading';
    try {
      const list = await this.#source.list();
      await this.#source.learnUsers(list.map((one) => one.createdBy).filter((id) => id !== ''));
      this.versions = list;
      this.status = 'ready';
    } catch {
      this.status = 'failed';
    }
  }

  /** Uploads `file` and makes it the file's new version; the version, if it was written. */
  async add(file: File): Promise<FileVersion | undefined> {
    const base = this.current;
    if (!base || this.writing) return undefined;
    this.#begin('upload');
    try {
      const upload = await this.#source.upload(file);
      if (!upload || upload === 'stopped') return undefined;
      try {
        return await this.#written(this.#source.add(base.version, upload));
      } catch (error) {
        await this.#refused(error);
        // What went up is still a file of the board: the reader may attach it on its own.
        if (this.refusal === 'stale') this.apart = upload;
        return undefined;
      }
    } finally {
      this.writing = undefined;
    }
  }

  /** Goes back to `version`; the new version, if it was written. */
  async restore(version: FileVersion): Promise<FileVersion | undefined> {
    const base = this.current;
    if (!base || this.writing) return undefined;
    this.#begin('restore');
    try {
      return await this.#written(this.#source.restore(version.version, base.version));
    } catch (error) {
      await this.#refused(error);
      return undefined;
    } finally {
      this.writing = undefined;
    }
  }

  /** Attaches the file that did not become a version as a file of its own; whether it was. */
  async attachApart(): Promise<boolean> {
    const upload = this.apart;
    if (!upload || this.writing) return false;
    this.writing = 'apart';
    try {
      await this.#source.attachApart(upload);
      this.dismiss();
      return true;
    } catch {
      this.refusal = 'failed';
      return false;
    } finally {
      this.writing = undefined;
    }
  }

  /** Lets what a refusal said, and the file it kept, go. */
  dismiss(): void {
    this.refusal = undefined;
    this.apart = undefined;
  }

  #begin(writing: 'upload' | 'restore'): void {
    this.writing = writing;
    this.dismiss();
  }

  async #written(writing: Promise<FileVersion>): Promise<FileVersion> {
    const version = await writing;
    await this.read();
    return version;
  }

  async #refused(error: unknown): Promise<void> {
    this.refusal = this.#source.refusalOf(error);
    // What the list said is not so any more.
    if (this.refusal === 'stale' || this.refusal === 'gone') await this.read();
  }
}
