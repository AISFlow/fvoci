/**
 * The compressed forms a static file is written in, and which of them a
 * client is sent.
 *
 * The build writes `<file>.br`, `<file>.zst` and `<file>.gz` next to a text
 * file (output.ts). A server that finds one of them sends it in place of the
 * file when the client's `Accept-Encoding` allows it. preview.ts and serve.ts
 * choose with `chooseEncoding`; the product's server has its own rule, and
 * the order here does not bind it.
 */

export interface Encoding {
  /** The name in `Accept-Encoding` and `Content-Encoding`. */
  name: 'br' | 'zstd' | 'gzip';
  /** What the build appends to the name of the file. */
  extension: '.br' | '.zst' | '.gz';
}

/**
 * In the order of preference: the first one the client accepts and the build
 * wrote is sent.
 *
 * Brotli comes first because its files are the smallest: every text file of
 * the build is smaller at brotli 11 than at zstd 19 (docs/build-notes.md has the
 * table), and for a file compressed once and sent
 * many times the bytes are what counts. Zstandard comes before gzip for the
 * same reason. To prefer zstd, move it to the front: nothing else reads the
 * order.
 */
export const ENCODINGS: readonly Encoding[] = [
  { name: 'br', extension: '.br' },
  { name: 'zstd', extension: '.zst' },
  { name: 'gzip', extension: '.gz' },
];

/**
 * Whether an `Accept-Encoding` header accepts a content coding: it lists the
 * coding, or `*`, with a weight above zero. A coding listed with `q=0` is
 * refused even when `*` would accept it.
 */
export function acceptsEncoding(header: string | string[] | undefined, name: string): boolean {
  const weights = new Map<string, number>();
  for (const part of [header ?? ''].flat().join(',').split(',')) {
    const [coding = '', ...parameters] = part.split(';').map((piece) => piece.trim());
    if (coding === '') continue;
    const weight = parameters
      .map((parameter) => /^q\s*=\s*([0-9.]+)$/i.exec(parameter)?.[1])
      .find((value) => value !== undefined);
    weights.set(coding.toLowerCase(), weight === undefined ? 1 : Number(weight));
  }
  const weight = weights.get(name) ?? weights.get('*') ?? 0;
  return weight > 0;
}

/**
 * The encoding to answer a request with: the first of `ENCODINGS` that the
 * header accepts and `has` a file for. Undefined means the file as it is.
 */
export function chooseEncoding(
  header: string | string[] | undefined,
  has: (extension: Encoding['extension']) => boolean,
): Encoding | undefined {
  return ENCODINGS.find(
    (encoding) => acceptsEncoding(header, encoding.name) && has(encoding.extension),
  );
}
