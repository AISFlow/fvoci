/**
 * Bodies in Markdown and containers as ZIPs of Markdown files in the
 * in-memory backend (`Backend.markdown`): a body is read as it is now, as its
 * own read refuses it (`card`, `wiki.page`, `course.unit`), and written by
 * `markdownOf`; a container is for its members, as its archive is, one file
 * of each card (no template), page, unit and entry, side by side in a folder
 * of its title, named by theirs. The server's export also has the tree, the
 * files and `board.csv`.
 */
import { storedZip, type ZipEntry } from '$lib/logic/zip-store.ts';

import type { CardDetail, DownloadedFile, DownloadOptions } from '../backend.ts';
import type { CourseUnitDetail } from '../course-types.ts';
import type { MarkdownTarget, Markdowns } from '../markdown-types.ts';
import type { WikiPageDetail } from '../wiki-types.ts';

import { flushContainers } from './bodies.ts';
import { containerInWorld, memberIdsOf } from './container-ops.ts';
import { markdownOf } from './markdown-writer.ts';
import { fail } from './refuse.ts';
import type { World } from './world.ts';

interface Asker {
  readonly world: World;
  readonly userId: string;
  card(boardId: string, cardId: string): Promise<CardDetail>;
  readonly wiki: { page(wikiId: string, pageId: string): Promise<WikiPageDetail> };
  readonly course: { unit(courseId: string, unitId: string): Promise<CourseUnitDetail> };
}

/** What a file is called whose title gives no name, by what it is of. */
const UNTITLED: Record<string, string> = {
  card: 'card',
  page: 'page',
  unit: 'unit',
  entry: 'entry',
};

/** `title` as a name of a file: nothing a system refuses or a link reads otherwise. */
function nameOf(title: string, fallback: string): string {
  // eslint-disable-next-line no-control-regex -- control characters are what is taken out
  const name = title.replace(/[\u0000-\u001f\u007f/\\:*?"<>|#%]/g, '_').trim();
  return name.replace(/^\.+|\.+$/g, '') || fallback;
}

export class MemoryMarkdowns implements Markdowns {
  readonly #asker: Asker;
  readonly #enter: (signal?: AbortSignal) => Promise<void>;

  constructor(asker: Asker, enter: (signal?: AbortSignal) => Promise<void>) {
    this.#asker = asker;
    this.#enter = enter;
  }

  async body(target: MarkdownTarget): Promise<string> {
    const asker = this.#asker;
    // What the rooms hold is written first, as the server does: what is taken out is what is shown.
    const containerId =
      target.kind === 'card'
        ? target.boardId
        : target.kind === 'page'
          ? target.wikiId
          : target.courseId;
    if (asker.world.containers.has(containerId))
      flushContainers(asker.world, [{ id: containerId }]);
    switch (target.kind) {
      case 'card':
        return markdownOf((await asker.card(target.boardId, target.cardId)).body);
      case 'page':
        return markdownOf((await asker.wiki.page(target.wikiId, target.pageId)).body);
      case 'unit':
        return markdownOf((await asker.course.unit(target.courseId, target.unitId)).body);
    }
  }

  async exportContainer(containerId: string, options?: DownloadOptions): Promise<DownloadedFile> {
    await this.#enter(options?.signal);
    const { world, userId } = this.#asker;
    const container = containerInWorld(world, containerId);
    if (!memberIdsOf(world, containerId).includes(userId)) fail(403, 'not a member of the board');
    flushContainers(world, [container]);
    const root = nameOf(container.title, container.kind ?? 'board');
    const taken = new Set<string>();
    const encoder = new TextEncoder();
    const entries: ZipEntry[] = [];
    for (const block of world.blocksOf(containerId)) {
      const fallback = UNTITLED[block.type];
      if (fallback === undefined || block.fields.isTemplate === true) continue;
      const stem = nameOf(block.title, fallback);
      let name = `${stem}.md`;
      for (let count = 2; taken.has(name.toLowerCase()); count += 1) {
        name = `${stem} (${String(count)}).md`;
      }
      taken.add(name.toLowerCase());
      const heading = block.title.trim() === '' ? '' : `# ${block.title.trim()}\n\n`;
      const text = `${heading}${markdownOf(block.body)}\n`;
      entries.push({ name: `${root}/${name}`, bytes: encoder.encode(text) });
    }
    const day = new Date().toISOString().slice(0, 10);
    const bytes = new Blob([storedZip(entries)], { type: 'application/zip' });
    options?.onProgress?.({ sentBytes: bytes.size, totalBytes: bytes.size });
    return { bytes, name: `markdown-${day}.zip` };
  }
}
