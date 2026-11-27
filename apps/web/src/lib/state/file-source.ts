/**
 * The files the bodies on screen name, from the backend: the app hands this
 * to the editor's file source (`editor/files.ts`) for the board or wiki that
 * is shown, which is the only one whose files a body may name. Their bytes
 * are read to save a file under its name and to view it.
 */
import type { FileDetails, FileSource } from '../editor/files.ts';
import type { Backend } from './backend.ts';
import { isBackendError } from './problems.ts';

/** What of the backend a file source reads. */
type FileReads = Pick<Backend, 'fileUrl' | 'fileDetails' | 'fileBytes'>;

/**
 * The files of the board or wiki `shown` names now; none while it names
 * none. A file the server has no record of there (gone, or another's) is
 * `null`, as the editor's source has it. Bytes that cannot be read are told
 * to `report`, as an attachment's are, and refused.
 */
export function fileSourceOf(
  backend: FileReads,
  shown: () => string | undefined,
  report: (error: unknown) => void,
): FileSource {
  return {
    scope: shown,
    url: (fileId) => {
      const containerId = shown();
      return containerId === undefined ? undefined : backend.fileUrl(containerId, fileId);
    },
    details: async (fileId): Promise<FileDetails | null> => {
      const containerId = shown();
      if (containerId === undefined) throw new Error('no board or wiki is shown');
      try {
        return await backend.fileDetails(containerId, fileId);
      } catch (error) {
        if (isBackendError(error) && error.status === 404) return null;
        throw error;
      }
    },
    bytes: async (fileId, signal) => {
      const containerId = shown();
      if (containerId === undefined) throw new Error('no board or wiki is shown');
      try {
        return await backend.fileBytes(containerId, fileId, signal);
      } catch (error) {
        // Bytes no longer wanted (the viewer closed) are no failure to tell.
        if (!signal?.aborted) report(error);
        throw error;
      }
    },
  };
}
