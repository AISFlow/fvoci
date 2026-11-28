/**
 * Bodies in Markdown, for another tool (`format=markdown` of `getCard`,
 * `getPage` and `getUnit`), and a whole board, wiki, course or diary as a ZIP
 * of Markdown files (`exportContainerMarkdown`). The server writes both, as an
 * agent reads a body: what Markdown does not hold is written as near as it
 * goes. None through a public link.
 */
import type { DownloadedFile, DownloadOptions } from './backend.ts';

/** The card, page or unit whose body is asked for. */
export type MarkdownTarget =
  | { readonly kind: 'card'; readonly boardId: string; readonly cardId: string }
  | { readonly kind: 'page'; readonly wikiId: string; readonly pageId: string }
  | { readonly kind: 'unit'; readonly courseId: string; readonly unitId: string };

export interface Markdowns {
  /** The body of `target` in Markdown; empty for none. */
  body(target: MarkdownTarget): Promise<string>;
  /**
   * The container as a ZIP of Markdown files (`markdown-YYYY-MM-DD.zip`), for
   * those the archive is for: its members, a diary's owner alone.
   */
  exportContainer(containerId: string, options?: DownloadOptions): Promise<DownloadedFile>;
}

/** What reads the body of `target` in Markdown; none where there is no Markdown. */
export function markdownReader(
  markdowns: Markdowns | undefined,
  target: MarkdownTarget,
): (() => Promise<string>) | undefined {
  return markdowns && (() => markdowns.body(target));
}

/** Where a backend writes Markdown; none through a public link. */
export interface MarkdownOfBackend {
  readonly markdown: Markdowns | undefined;
}
