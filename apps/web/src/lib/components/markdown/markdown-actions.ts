/**
 * The entries of the menu of a card, a page or a unit that take its body out
 * as Markdown, for another tool: copied, or saved as a `.md` file. Its title
 * is the heading, then its body as it is read (`read`: the backend's
 * Markdown, which the server writes). None where nothing reads it, as
 * through a public link.
 */
import ClipboardCopyIcon from '@lucide/svelte/icons/clipboard-copy';
import FileDownIcon from '@lucide/svelte/icons/file-down';

import type { MenuAction } from '$lib/components/menu/actions.ts';
import { m } from '$lib/i18n/paraglide/messages.js';
import { fileName, saveBlob } from '$lib/save.ts';

/** What the entries are of. */
export interface MarkdownOf {
  /** Reads the body in Markdown; none where there is no Markdown. */
  readonly read: (() => Promise<string>) | undefined;
  /** Its title as it is shown; empty for none. */
  readonly title: string;
  /** Said once the Markdown is on the clipboard. */
  readonly onCopied: () => void;
  /** Says why the body could not be read. */
  readonly onRefused: (error: unknown) => void;
}

/** The title as a heading, then the body, ending in a new line. */
export function withHeading(title: string, body: string): string {
  const heading = title.replace(/\s+/g, ' ').trim();
  const head = heading === '' ? '' : `# ${heading}`;
  const text = head === '' || body === '' ? head + body : `${head}\n\n${body}`;
  return text.endsWith('\n') ? text : `${text}\n`;
}

/**
 * Puts `text` on the clipboard as it arrives: the browser is given the
 * promise of it at once, while the click still counts, as Safari wants.
 */
async function copyWhenRead(text: Promise<string>): Promise<void> {
  const clipboard = navigator.clipboard;
  if (typeof ClipboardItem === 'undefined' || typeof clipboard.write !== 'function') {
    await clipboard.writeText(await text);
    return;
  }
  const plain = text.then((read) => new Blob([read], { type: 'text/plain' }));
  await clipboard.write([new ClipboardItem({ 'text/plain': plain })]);
}

/** The entries that copy it as Markdown and save it as a `.md` file; none where there is no Markdown. */
export function markdownActions(of: MarkdownOf): MenuAction[] {
  const reader = of.read;
  if (!reader) return [];
  const read = async (): Promise<string> => withHeading(of.title, await reader());
  const copy = async (): Promise<void> => {
    const text = read();
    try {
      await copyWhenRead(text);
    } catch {
      // Not read: say why. No clipboard (a page not served securely): nothing to say.
      await text.then(() => undefined, of.onRefused);
      return;
    }
    of.onCopied();
  };
  const save = async (): Promise<void> => {
    let text: string;
    try {
      text = await read();
    } catch (error) {
      of.onRefused(error);
      return;
    }
    const name = fileName(of.title, '.md', m.markdown_untitled());
    saveBlob(new Blob([text], { type: 'text/markdown;charset=utf-8' }), name);
  };
  return [
    {
      id: 'copy-markdown',
      label: m.markdown_copy(),
      icon: ClipboardCopyIcon,
      run: () => void copy(),
    },
    { id: 'save-markdown', label: m.markdown_save(), icon: FileDownIcon, run: () => void save() },
  ];
}
