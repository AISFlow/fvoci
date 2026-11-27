/**
 * The files of the backend in memory: an upload, taken in parts as the
 * server takes a large one, and a file read through its board or wiki only.
 */
import type { UploadedFile, UploadOptions } from '../backend.ts';

import { memberRole } from './container-access.ts';
import { fail } from './refuse.ts';
import { LIMITS } from './shapes.ts';
import type { StoredFile, World } from './world.ts';

/**
 * Whether a name is one the server takes for a file: at most 255
 * characters, without U+0000, and with a last segment, parted by `/` or
 * `\`, that is neither blank nor `.` or `..`. Blank is what the server
 * calls so (Unicode's `White_Space`, which is not quite what `trim` takes).
 */
function fitsAFile(name: string): boolean {
  const last = name.split(/[/\\]/).at(-1) ?? '';
  return (
    Array.from(name).length <= 255 &&
    !name.includes('\0') &&
    !/^\p{White_Space}*$/u.test(last) &&
    last !== '.' &&
    last !== '..'
  );
}

/**
 * Stores `file` with the board or wiki `containerId` as its reader's upload,
 * telling the progress after each part; `enter` is the backend's way into a
 * call, asked before each part (where a test holds or fails it, and a signal
 * gives it up).
 */
export async function storeUpload(
  { world, userId }: { readonly world: World; readonly userId: string },
  containerId: string,
  file: File,
  options: UploadOptions,
  enter: (signal?: AbortSignal) => Promise<void>,
): Promise<UploadedFile> {
  if (!fitsAFile(file.name)) fail(400, 'the name is none of a file');
  if (file.size > LIMITS.fileBytes) {
    fail(413, 'the file is too large', { limit: LIMITS.fileBytes, reason: 'file_too_large' });
  }
  const parts = Math.max(1, Math.ceil(file.size / LIMITS.uploadPartBytes));
  for (let part = 1; part <= parts; part += 1) {
    await enter(options.signal);
    options.onProgress?.({
      sentBytes: Math.min(file.size, part * LIMITS.uploadPartBytes),
      totalBytes: file.size,
    });
  }
  const stored = {
    fileId: world.newId(),
    containerId,
    // What the server keeps of a name: what follows its last `/`.
    name: file.name.slice(file.name.lastIndexOf('/') + 1),
    size: file.size,
    contentType: file.type || 'application/octet-stream',
    blob: file as Blob,
  };
  world.files.set(stored.fileId, { ...stored, createdAt: world.now(), createdBy: userId });
  const { blob: _blob, containerId: _containerId, ...answer } = stored;
  return answer;
}

/** A file of the board or wiki, for whoever may read it; 404 for one of anything else. */
export function readableFile(
  world: World,
  userId: string,
  containerId: string,
  fileId: string,
): StoredFile {
  memberRole(world, userId, containerId, 'any');
  const file = world.files.get(fileId);
  if (file?.containerId !== containerId) fail(404, 'no such file');
  return file;
}

/** The bytes of such a file; 404 for one the seed only names. */
export function readableBlob(
  world: World,
  userId: string,
  containerId: string,
  fileId: string,
): Blob {
  const { blob } = readableFile(world, userId, containerId, fileId);
  if (!blob) fail(404, 'no such file');
  return blob;
}
