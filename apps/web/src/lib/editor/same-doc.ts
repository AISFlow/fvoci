/**
 * Structural equality for ProseMirror `DocNode` trees: short-circuits on
 * reference identity and exits on the first mismatch without serializing the
 * entire document via `JSON.stringify`.
 */
import type { DocMark, DocNode } from './schema.ts';

function sameValue(a: unknown, b: unknown): boolean {
  if (Object.is(a, b)) return true;
  if (typeof a !== typeof b || a === null || b === null) return false;
  if (Array.isArray(a)) {
    if (!Array.isArray(b) || a.length !== b.length) return false;
    for (let i = 0; i < a.length; i += 1) {
      if (!sameValue(a[i], b[i])) return false;
    }
    return true;
  }
  if (typeof a === 'object') {
    return sameRecord(a as Record<string, unknown>, b as Record<string, unknown>);
  }
  return false;
}

function sameRecord(
  a: Record<string, unknown> | undefined,
  b: Record<string, unknown> | undefined,
): boolean {
  if (a === b) return true;
  if (a === undefined || b === undefined) return false;
  let countA = 0;
  for (const key of Object.keys(a)) {
    const valA = a[key];
    if (valA === undefined) continue;
    countA += 1;
    if (!(key in b) || b[key] === undefined || !sameValue(valA, b[key])) return false;
  }
  let countB = 0;
  for (const key of Object.keys(b)) {
    if (b[key] !== undefined) countB += 1;
  }
  return countA === countB;
}

function sameMarks(a: readonly DocMark[] | undefined, b: readonly DocMark[] | undefined): boolean {
  if (a === b) return true;
  if (a?.length !== b?.length || !a || !b) return false;
  for (let i = 0; i < a.length; i += 1) {
    const left = a[i];
    const right = b[i];
    if (left?.type !== right?.type || !left || !right || !sameRecord(left.attrs, right.attrs)) {
      return false;
    }
  }
  return true;
}

export function sameDocNode(first: DocNode | null, second: DocNode | null): boolean {
  if (first === second) return true;
  if (first === null || second === null) return false;
  if (first.type !== second.type || first.text !== second.text) return false;
  if (!sameRecord(first.attrs, second.attrs)) return false;
  if (!sameMarks(first.marks, second.marks)) return false;
  const contentA = first.content;
  const contentB = second.content;
  if (contentA === contentB) return true;
  if (contentA?.length !== contentB?.length || !contentA || !contentB) {
    return false;
  }
  for (let i = 0; i < contentA.length; i += 1) {
    const childA = contentA[i];
    const childB = contentB[i];
    if (!childA || !childB || !sameDocNode(childA, childB)) return false;
  }
  return true;
}
