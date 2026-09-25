const TARGET = 1500;
const MIN = 800;
const MAX = 2000;
const OVERLAP = 150;

export interface TextChunk {
  chunkNo: number;
  start: number;
  end: number;
  text: string;
}

/** Source `chunkPlainText`: UTF-16 `String.length` offsets, paragraph/page boundaries first. */
function boundaries(text: string): number[] {
  return [...text.matchAll(/(?:\f|\n[ \t]*\n)\s*/g)].map((m) => (m.index ?? 0) + m[0].length);
}

export function chunkPlainText(text: string): TextChunk[] {
  if (text.length === 0) return [];
  const bounds = boundaries(text);
  const chunks: TextChunk[] = [];
  let start = 0;
  while (start < text.length) {
    let end = text.length;
    if (end - start > MAX) {
      const goal = start + TARGET;
      const near = bounds.filter((b) => b >= start + MIN && b <= start + MAX);
      end =
        near.length === 0
          ? goal
          : near.reduce((best, b) => (Math.abs(b - goal) < Math.abs(best - goal) ? b : best));
    }
    chunks.push({
      chunkNo: chunks.length,
      start,
      end,
      text: text.slice(start, end),
    });
    if (end >= text.length) break;
    start = end - OVERLAP;
  }
  return chunks;
}
