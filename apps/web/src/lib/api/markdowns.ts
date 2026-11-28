/**
 * Bodies in Markdown and containers as ZIPs of Markdown files, on the server
 * (`getCard`, `getPage`, `getUnit` with `format=markdown`, and
 * `exportContainerMarkdown`).
 */
import type { DownloadedFile, DownloadOptions } from '../state/backend.ts';
import type { MarkdownTarget, Markdowns } from '../state/markdown-types.ts';

import type { RequestOptions } from './http.ts';
import type { WorkspacePath } from './requests.ts';

/** A request under the workspace of the container that `params.containerId` names. */
type Ask = <T>(
  path: WorkspacePath,
  params: Record<string, string>,
  options: RequestOptions,
) => Promise<T>;

/** The download of the ZIP of the container `containerId`. */
type Download = (containerId: string, options?: DownloadOptions) => Promise<DownloadedFile>;

/** What a read with `format=markdown` answers, of what this needs. */
interface InMarkdown {
  bodyMarkdown?: string;
}

const AS_MARKDOWN: RequestOptions = { query: { format: 'markdown' } };

export class ServerMarkdowns implements Markdowns {
  readonly #ask: Ask;
  readonly #download: Download;

  constructor(ask: Ask, download: Download) {
    this.#ask = ask;
    this.#download = download;
  }

  async body(target: MarkdownTarget): Promise<string> {
    let read: InMarkdown;
    switch (target.kind) {
      case 'card':
        read = await this.#ask(
          '/cards/{cardId}',
          { containerId: target.boardId, cardId: target.cardId },
          AS_MARKDOWN,
        );
        break;
      case 'page':
        read = await this.#ask(
          '/wikis/{wikiId}/pages/{pageId}',
          { containerId: target.wikiId, wikiId: target.wikiId, pageId: target.pageId },
          AS_MARKDOWN,
        );
        break;
      case 'unit':
        read = await this.#ask(
          '/courses/{courseId}/units/{unitId}',
          { containerId: target.courseId, courseId: target.courseId, unitId: target.unitId },
          AS_MARKDOWN,
        );
        break;
    }
    return read.bodyMarkdown ?? '';
  }

  exportContainer(containerId: string, options?: DownloadOptions): Promise<DownloadedFile> {
    return this.#download(containerId, options);
  }
}
