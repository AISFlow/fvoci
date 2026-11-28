/**
 * Enforces domain attribute invariants on collaborative ProseMirror nodes and
 * marks (`H-7`) without mutating `Y.Doc`: ProseMirror's schema omits
 * `AttributeSpec.validate` so `yXmlFragmentToProseMirrorRootNode` preserves
 * unknown or unvalidated remote content (`contracts/ydoc-vectors.json`), while
 * `CheckedCollaboration` pauses editing whenever an incoming Yjs transaction
 * carries invalid attributes.
 */
import type { Node as ProseMirrorNode } from '@tiptap/pm/model';

import {
  isAllowedHref,
  isBlockId,
  isCalloutTone,
  isFileId,
  isLanguage,
  isUserId,
  MAX_LATEX,
} from './schema.ts';
import { allowedSpan, MAX_TABLE_COLUMNS, MAX_TABLE_ROWS } from './table-grid.ts';

function checkNodeAttrs(node: ProseMirrorNode): void {
  const attrs = node.attrs as Record<string, unknown>;
  switch (node.type.name) {
    case 'orderedList': {
      const start = attrs.start;
      if (
        start !== undefined &&
        start !== null &&
        (typeof start !== 'number' ||
          !Number.isInteger(start) ||
          start < 0 ||
          start > 1_000_000_000)
      ) {
        throw new RangeError('Invalid orderedList start attribute');
      }
      break;
    }
    case 'codeBlock': {
      const language = attrs.language;
      if (language !== undefined && language !== null && !isLanguage(language)) {
        throw new RangeError('Invalid codeBlock language attribute');
      }
      break;
    }
    case 'image':
      if (!isFileId(attrs.fileId)) throw new RangeError('Invalid image fileId attribute');
      if (
        attrs.alt !== undefined &&
        attrs.alt !== null &&
        (typeof attrs.alt !== 'string' || attrs.alt.length > 1000)
      ) {
        throw new RangeError('Invalid image alt attribute');
      }
      break;
    case 'file':
      if (!isFileId(attrs.fileId)) throw new RangeError('Invalid file fileId attribute');
      break;
    case 'mention':
      if (!isUserId(attrs.userId)) throw new RangeError('Invalid mention userId attribute');
      break;
    case 'reference':
      if (!isBlockId(attrs.id)) throw new RangeError('Invalid reference id attribute');
      break;
    case 'inlineMath':
    case 'blockMath':
      if (
        typeof attrs.latex !== 'string' ||
        attrs.latex.length === 0 ||
        attrs.latex.length > MAX_LATEX
      ) {
        throw new RangeError(`Invalid ${node.type.name} latex attribute`);
      }
      break;
    case 'tableCell':
    case 'tableHeader':
      if (
        allowedSpan(attrs.colspan ?? 1, MAX_TABLE_COLUMNS) === undefined ||
        allowedSpan(attrs.rowspan ?? 1, MAX_TABLE_ROWS) === undefined
      ) {
        throw new RangeError(`Invalid ${node.type.name} span attribute`);
      }
      break;
    case 'callout':
      if (!isCalloutTone(attrs.tone)) throw new RangeError('Invalid callout tone attribute');
      break;
    default:
      break;
  }
  for (const mark of node.marks) {
    if (mark.type.name === 'link' && !isAllowedHref((mark.attrs as Record<string, unknown>).href)) {
      throw new RangeError('Invalid link href attribute');
    }
  }
}

export function checkSubtreeAttrs(root: ProseMirrorNode): void {
  checkNodeAttrs(root);
  root.descendants((node) => {
    checkNodeAttrs(node);
  });
}
