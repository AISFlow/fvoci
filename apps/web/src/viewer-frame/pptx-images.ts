/**
 * The size in pixels of a picture of a presentation, read from its header
 * in the PowerPoint worker: what the browser will decode it to, which the
 * file's name and the slide's XML do not say. Only the kinds a browser draws
 * in an `img` without running anything are taken (PNG, JPEG, GIF, BMP,
 * WebP); a picture of another kind (EMF, WMF, TIFF, SVG) or whose header
 * cannot be read is left out of what the renderer gets.
 */

export interface Pixels {
  width: number;
  height: number;
}

const PNG = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

const startsWith = (bytes: Uint8Array, expected: readonly number[], at = 0): boolean =>
  expected.every((byte, index) => bytes[at + index] === byte);

const ascii = (bytes: Uint8Array, at: number, length: number): string =>
  String.fromCharCode(...bytes.subarray(at, at + length));

function png(view: DataView): Pixels {
  // The header chunk comes first: length, "IHDR", width, height.
  return { width: view.getUint32(16), height: view.getUint32(20) };
}

/** The largest frame a JPEG declares (a start-of-frame marker), or undefined when it has none. */
function jpeg(bytes: Uint8Array, view: DataView): Pixels | undefined {
  let found: Pixels | undefined;
  let at = 2;
  while (at + 9 <= bytes.length) {
    if (bytes[at] !== 0xff) return found;
    const marker = bytes[at + 1] ?? 0;
    if (marker === 0xff) {
      at += 1;
      continue;
    }
    // Markers without a length: the start of a scan's data ends the headers.
    if (marker === 0xd8 || (marker >= 0xd0 && marker <= 0xd7) || marker === 0x01) {
      at += 2;
      continue;
    }
    if (marker === 0xda || marker === 0xd9) return found;
    const length = view.getUint16(at + 2);
    const isFrame = marker >= 0xc0 && marker <= 0xcf && ![0xc4, 0xc8, 0xcc].includes(marker);
    if (isFrame) {
      const frame = { width: view.getUint16(at + 7), height: view.getUint16(at + 5) };
      if (!found || frame.width * frame.height > found.width * found.height) found = frame;
    }
    at += 2 + length;
  }
  return found;
}

/** Skips a run of data sub-blocks (each a length byte, then that many bytes, until a 0). */
function skipBlocks(bytes: Uint8Array, at: number): number {
  let position = at;
  while (position < bytes.length && bytes[position] !== 0) position += 1 + (bytes[position] ?? 0);
  return position + 1;
}

/**
 * The screen a GIF declares, grown to the frames it holds: a frame may lie
 * past the screen, and a decoder can take the larger.
 */
function gif(bytes: Uint8Array, view: DataView): Pixels {
  let width = view.getUint16(6, true);
  let height = view.getUint16(8, true);
  const flags = bytes[10] ?? 0;
  let at = 13 + (flags & 0x80 ? 3 * 2 ** ((flags & 7) + 1) : 0);
  while (at < bytes.length) {
    const block = bytes[at];
    if (block === 0x21) {
      at = skipBlocks(bytes, at + 2);
    } else if (block === 0x2c && at + 10 <= bytes.length) {
      width = Math.max(width, view.getUint16(at + 1, true) + view.getUint16(at + 5, true));
      height = Math.max(height, view.getUint16(at + 3, true) + view.getUint16(at + 7, true));
      const local = bytes[at + 9] ?? 0;
      at += 10 + (local & 0x80 ? 3 * 2 ** ((local & 7) + 1) : 0);
      at = skipBlocks(bytes, at + 1);
    } else {
      break;
    }
  }
  return { width, height };
}

function webp(bytes: Uint8Array, view: DataView): Pixels | undefined {
  const chunk = ascii(bytes, 12, 4);
  if (chunk === 'VP8X') {
    const width = 1 + (view.getUint32(24, true) & 0xffffff);
    const height = 1 + (view.getUint32(27, true) & 0xffffff);
    return { width, height };
  }
  if (chunk === 'VP8L') {
    const bits = view.getUint32(21, true);
    return { width: 1 + (bits & 0x3fff), height: 1 + ((bits >> 14) & 0x3fff) };
  }
  if (chunk === 'VP8 ') {
    return { width: view.getUint16(26, true) & 0x3fff, height: view.getUint16(28, true) & 0x3fff };
  }
  return undefined;
}

/** The size the picture decodes to, or undefined when it is no picture of the kinds taken. */
export function pixelsOf(bytes: Uint8Array): Pixels | undefined {
  if (bytes.length < 32) return undefined;
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  if (startsWith(bytes, PNG) && ascii(bytes, 12, 4) === 'IHDR') return png(view);
  if (startsWith(bytes, [0xff, 0xd8])) return jpeg(bytes, view);
  const head = ascii(bytes, 0, 6);
  if (head === 'GIF87a' || head === 'GIF89a') return gif(bytes, view);
  if (head.startsWith('BM')) {
    return { width: Math.abs(view.getInt32(18, true)), height: Math.abs(view.getInt32(22, true)) };
  }
  if (ascii(bytes, 0, 4) === 'RIFF' && ascii(bytes, 8, 4) === 'WEBP') return webp(bytes, view);
  return undefined;
}
