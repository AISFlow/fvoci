/**
 * The two things the font tool has to read out of a TrueType or OpenType
 * file itself: which characters it covers, and its name strings.
 */

interface TableRecord {
  offset: number;
  length: number;
}

function tables(font: Buffer): Map<string, TableRecord> {
  const count = font.readUInt16BE(4);
  const records = new Map<string, TableRecord>();
  for (let index = 0; index < count; index += 1) {
    const at = 12 + index * 16;
    records.set(font.toString('latin1', at, at + 4), {
      offset: font.readUInt32BE(at + 8),
      length: font.readUInt32BE(at + 12),
    });
  }
  return records;
}

function table(font: Buffer, tag: string): Buffer {
  const record = tables(font).get(tag);
  if (!record) throw new Error(`the font has no ${tag} table`);
  return font.subarray(record.offset, record.offset + record.length);
}

function readFormat12(sub: Buffer): number[] {
  const groups = sub.readUInt32BE(12);
  const codePoints: number[] = [];
  for (let index = 0; index < groups; index += 1) {
    const at = 16 + index * 12;
    const start = sub.readUInt32BE(at);
    const end = sub.readUInt32BE(at + 4);
    const firstGlyph = sub.readUInt32BE(at + 8);
    for (let code = start; code <= end; code += 1) {
      if (firstGlyph + (code - start) !== 0) codePoints.push(code);
    }
  }
  return codePoints;
}

function readFormat4(sub: Buffer): number[] {
  const segments = sub.readUInt16BE(6) / 2;
  const ends = 14;
  const starts = ends + segments * 2 + 2;
  const deltas = starts + segments * 2;
  const rangeOffsets = deltas + segments * 2;
  const codePoints: number[] = [];
  for (let index = 0; index < segments; index += 1) {
    const end = sub.readUInt16BE(ends + index * 2);
    const start = sub.readUInt16BE(starts + index * 2);
    const delta = sub.readUInt16BE(deltas + index * 2);
    const rangeOffsetAt = rangeOffsets + index * 2;
    const rangeOffset = sub.readUInt16BE(rangeOffsetAt);
    for (let code = start; code <= end && code !== 0xffff; code += 1) {
      let glyph: number;
      if (rangeOffset === 0) {
        glyph = (code + delta) & 0xffff;
      } else {
        const raw = sub.readUInt16BE(rangeOffsetAt + rangeOffset + (code - start) * 2);
        glyph = raw === 0 ? 0 : (raw + delta) & 0xffff;
      }
      if (glyph !== 0) codePoints.push(code);
    }
  }
  return codePoints;
}

/** Every code point the font maps to a glyph, ascending. */
export function readCodePoints(font: Buffer): number[] {
  const cmap = table(font, 'cmap');
  const count = cmap.readUInt16BE(2);
  let full: Buffer | undefined;
  let basic: Buffer | undefined;
  for (let index = 0; index < count; index += 1) {
    const at = 4 + index * 8;
    const platform = cmap.readUInt16BE(at);
    const encoding = cmap.readUInt16BE(at + 2);
    const sub = cmap.subarray(cmap.readUInt32BE(at + 4));
    const format = sub.readUInt16BE(0);
    const unicode = platform === 0 || (platform === 3 && (encoding === 1 || encoding === 10));
    if (!unicode) continue;
    if (format === 12) full = sub;
    else if (format === 4) basic = sub;
  }
  const codePoints = full ? readFormat12(full) : basic ? readFormat4(basic) : undefined;
  if (!codePoints) throw new Error('the font has no Unicode character map this tool can read');
  return [...new Set(codePoints)].sort((a, b) => a - b);
}

/** A string of the `name` table (5 is the version), from its Windows Unicode records. */
export function readName(font: Buffer, nameId: number): string | undefined {
  const name = table(font, 'name');
  const count = name.readUInt16BE(2);
  const storage = name.readUInt16BE(4);
  for (let index = 0; index < count; index += 1) {
    const at = 6 + index * 12;
    const platform = name.readUInt16BE(at);
    const language = name.readUInt16BE(at + 4);
    if (platform !== 3 || language !== 0x0409 || name.readUInt16BE(at + 6) !== nameId) continue;
    const length = name.readUInt16BE(at + 8);
    const offset = storage + name.readUInt16BE(at + 10);
    const bytes = Buffer.from(name.subarray(offset, offset + length));
    return bytes.swap16().toString('utf16le');
  }
  return undefined;
}
