/**
 * What a gallery shows of a card's body: the file of its first image and
 * the beginning of its text, made as the server makes `CardPreview`
 * (`contracts/openapi.json`). The memory backend makes it so, and so does a
 * card read whole with its body.
 */
import { plainText } from './body-text.ts';
import type { DocNode } from './schema.ts';

/** The longest text of a preview, in UTF-16 code units. */
export const PREVIEW_TEXT_LENGTH = 200;

export interface CardPreview {
  /** Empty: the body has no image. */
  imageFileId: string;
  /** Empty: the body has no text. */
  text: string;
}

/** The preview of a body; that of no body is empty. */
export function cardPreviewOf(body: DocNode | null | undefined): CardPreview {
  if (!body) return { imageFileId: '', text: '' };
  return { imageFileId: firstImage(body) ?? '', text: cut(plainText(withoutMentions(body))) };
}

/** The file of the first image in document order, also inside lists and tables. */
function firstImage(node: DocNode): string | undefined {
  if (node.type === 'image' && typeof node.attrs?.fileId === 'string') return node.attrs.fileId;
  for (const child of node.content ?? []) {
    const found = firstImage(child);
    if (found !== undefined) return found;
  }
  return undefined;
}

/** A mention gives nothing to the text: the server does not keep the name. */
function withoutMentions(node: DocNode): DocNode {
  if (!node.content) return node;
  return {
    ...node,
    content: node.content.filter((child) => child.type !== 'mention').map(withoutMentions),
  };
}

/** Made when first needed: a browser without it fails at a preview, not at loading the app. */
let graphemes: Intl.Segmenter | undefined;

/** At most the length, cut between characters as they are seen, without white space at its end. */
function cut(text: string): string {
  let units = 0;
  let end = text.length;
  graphemes ??= new Intl.Segmenter(undefined, { granularity: 'grapheme' });
  for (const { segment, index } of graphemes.segment(text)) {
    units += segment.length;
    if (units > PREVIEW_TEXT_LENGTH) {
      end = index;
      break;
    }
  }
  return text.slice(0, end).trimEnd();
}
