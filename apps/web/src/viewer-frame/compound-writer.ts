/**
 * A compound file (OLE) written anew from its streams (compound-guard.ts):
 * what the renderer reads is a file the frame wrote, of only the streams it
 * counted, laid out plainly whatever the file it came from did. Streams under
 * 4096 bytes go in the mini stream, the others in sectors of their own, each
 * chain in order, the siblings of a storage in a chain sorted as the format
 * sorts them, with DIFAT sectors when the FAT takes more than the header
 * lists.
 */

/** A stream: the names of the storages on its way and its own, and its bytes. */
export interface CompoundPart {
  names: readonly string[];
  data: Uint8Array;
}

const SIGNATURE = [0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1];
const MINI_SECTOR = 64;
const MINI_CUTOFF = 4096;
const END = 0xfffffffe;
const FREE = 0xffffffff;
const FAT_SECTOR = 0xfffffffd;
const DIFAT_SECTOR = 0xfffffffc;
const NONE = 0xffffffff;
/** The FAT sectors the header lists; DIFAT sectors list the rest. */
const HEADER_FAT_SECTORS = 109;
const ENTRY_SIZE = 128;
/** The most UTF-16 units a name may have, its terminating zero apart. */
const NAME_LIMIT = 31;

const ROOT = 5;
const STORAGE = 1;
const STREAM = 2;

interface Node {
  name: string;
  type: typeof ROOT | typeof STORAGE | typeof STREAM;
  children: Map<string, Node>;
  data?: Uint8Array;
  id: number;
  start: number;
  size: number;
  child: number;
  right: number;
}

/** Names in the order the format sorts siblings: shorter first, then by upper case. */
function compare(a: string, b: string): number {
  if (a.length !== b.length) return a.length - b.length;
  const [upperA, upperB] = [a.toUpperCase(), b.toUpperCase()];
  return upperA < upperB ? -1 : upperA > upperB ? 1 : 0;
}

const node = (name: string, type: Node['type']): Node => ({
  name,
  type,
  children: new Map(),
  id: 0,
  start: END,
  size: 0,
  child: NONE,
  right: NONE,
});

/** The entries in the order of the directory, the root first; throws on a name used twice. */
function tree(parts: readonly CompoundPart[]): Node[] {
  const root = node('Root Entry', ROOT);
  for (const { names, data } of parts) {
    let at = root;
    names.forEach((name, index) => {
      if (name.length === 0 || name.length > NAME_LIMIT) throw new Error(`a bad name: ${name}`);
      const leaf = index === names.length - 1;
      const key = name.toUpperCase();
      const next = at.children.get(key) ?? node(name, leaf ? STREAM : STORAGE);
      if ((next.type === STREAM) !== leaf || next.data)
        throw new Error(`a name used twice: ${name}`);
      if (leaf) next.data = data;
      at.children.set(key, next);
      at = next;
    });
  }
  const nodes: Node[] = [];
  const visit = (each: Node): void => {
    each.id = nodes.length;
    nodes.push(each);
    const children = [...each.children.values()].sort((a, b) => compare(a.name, b.name));
    children.forEach(visit);
    // A chain of siblings in order, each the right of the one before: a valid tree.
    each.child = children[0]?.id ?? NONE;
    children.forEach((child, index) => {
      child.right = children[index + 1]?.id ?? NONE;
    });
  };
  visit(root);
  return nodes;
}

/** Sectors of `size` holding `data`, and their chain in `table`; the first sector, or END. */
function place(sectors: Uint8Array[], table: number[], data: Uint8Array, size: number): number {
  const count = Math.ceil(data.length / size);
  if (count === 0) return END;
  const first = sectors.length;
  for (let index = 0; index < count; index += 1) {
    const part = data.subarray(index * size, (index + 1) * size);
    if (part.length === size) {
      sectors.push(part);
    } else {
      const sector = new Uint8Array(size);
      sector.set(part);
      sectors.push(sector);
    }
    table.push(index === count - 1 ? END : first + index + 1);
  }
  return first;
}

/** A table of numbers as bytes, in whole sectors of `size`, the rest free. */
function tableBytes(table: readonly number[], size: number): Uint8Array {
  const out = new Uint8Array(Math.ceil((table.length * 4) / size) * size).fill(0xff);
  const view = new DataView(out.buffer);
  table.forEach((next, index) => {
    view.setUint32(index * 4, next, true);
  });
  return out;
}

/** The directory, in whole sectors of `size`; the entries past the nodes unused. */
function directory(nodes: readonly Node[], size: number): Uint8Array {
  const perSector = size / ENTRY_SIZE;
  const out = new Uint8Array(Math.ceil(nodes.length / perSector) * perSector * ENTRY_SIZE);
  const view = new DataView(out.buffer);
  for (let index = 0; index < out.length / ENTRY_SIZE; index += 1) {
    const at = index * ENTRY_SIZE;
    const each = nodes[index];
    view.setUint32(at + 68, NONE, true);
    view.setUint32(at + 72, each?.right ?? NONE, true);
    view.setUint32(at + 76, each?.child ?? NONE, true);
    if (!each) continue;
    for (let unit = 0; unit < each.name.length; unit += 1) {
      view.setUint16(at + unit * 2, each.name.charCodeAt(unit), true);
    }
    view.setUint16(at + 64, (each.name.length + 1) * 2, true);
    out[at + 66] = each.type;
    out[at + 67] = 1;
    view.setUint32(at + 116, each.type === STORAGE ? 0 : each.start, true);
    view.setUint32(at + 120, each.size, true);
  }
  return out;
}

/** How many FAT and DIFAT sectors a file of `used` other sectors takes. */
function tableCounts(used: number, size: number): { fats: number; difats: number } {
  const perSector = size / 4;
  let [fats, difats] = [0, 0];
  for (;;) {
    const neededFats = Math.ceil((used + fats + difats) / perSector);
    const neededDifats = Math.ceil(Math.max(0, neededFats - HEADER_FAT_SECTORS) / (perSector - 1));
    if (neededFats === fats && neededDifats === difats) return { fats, difats };
    [fats, difats] = [neededFats, neededDifats];
  }
}

/** The FAT's sectors and the DIFAT's, which list the FAT sectors past the header's 109. */
function tableSectors(
  fat: number[],
  size: number,
  first: number,
  counts: { fats: number; difats: number },
): Uint8Array[] {
  const perSector = size / 4;
  for (let index = 0; index < counts.fats; index += 1) fat.push(FAT_SECTOR);
  for (let index = 0; index < counts.difats; index += 1) fat.push(DIFAT_SECTOR);
  const fatBytes = tableBytes(fat, size);
  const out: Uint8Array[] = [];
  for (let index = 0; index < counts.fats; index += 1) {
    out.push(fatBytes.subarray(index * size, (index + 1) * size));
  }
  const difatStart = first + counts.fats;
  for (let index = 0; index < counts.difats; index += 1) {
    const listed: number[] = [];
    for (let slot = 0; slot < perSector - 1; slot += 1) {
      const fatIndex = HEADER_FAT_SECTORS + index * (perSector - 1) + slot;
      listed.push(fatIndex < counts.fats ? first + fatIndex : FREE);
    }
    listed.push(index === counts.difats - 1 ? END : difatStart + index + 1);
    out.push(tableBytes(listed, size));
  }
  return out;
}

function header(
  size: number,
  version: 3 | 4,
  layout: { fats: number; difats: number; fatStart: number; directory: Uint8Array },
  starts: { directory: number; miniFat: number; miniFatSectors: number },
): Uint8Array {
  const out = new Uint8Array(size);
  out.set(SIGNATURE);
  const view = new DataView(out.buffer);
  view.setUint16(24, 0x3e, true);
  view.setUint16(26, version, true);
  view.setUint16(28, 0xfffe, true);
  view.setUint16(30, version === 3 ? 9 : 12, true);
  view.setUint16(32, 6, true);
  view.setUint32(40, version === 3 ? 0 : layout.directory.length / size, true);
  view.setUint32(44, layout.fats, true);
  view.setUint32(48, starts.directory, true);
  view.setUint32(56, MINI_CUTOFF, true);
  view.setUint32(60, starts.miniFat, true);
  view.setUint32(64, starts.miniFatSectors, true);
  view.setUint32(68, layout.difats > 0 ? layout.fatStart + layout.fats : END, true);
  view.setUint32(72, layout.difats, true);
  for (let index = 0; index < HEADER_FAT_SECTORS; index += 1) {
    const sector = index < layout.fats ? layout.fatStart + index : FREE;
    view.setUint32(76 + index * 4, sector, true);
  }
  return out;
}

/**
 * The compound file of `parts`: version 3 (sectors of 512 bytes, as HWP
 * files have) or 4 (of 4096). Throws on a name used twice in a storage, or a
 * name the format cannot hold.
 */
export function writeCompound(
  parts: readonly CompoundPart[],
  version: 3 | 4 = 3,
): Uint8Array<ArrayBuffer> {
  const size = version === 3 ? 512 : 4096;
  const nodes = tree(parts);
  const sectors: Uint8Array[] = [];
  const fat: number[] = [];
  const miniParts: Uint8Array[] = [];
  const miniFat: number[] = [];
  for (const leaf of nodes) {
    if (!leaf.data) continue;
    leaf.size = leaf.data.length;
    leaf.start =
      leaf.size < MINI_CUTOFF
        ? place(miniParts, miniFat, leaf.data, MINI_SECTOR)
        : place(sectors, fat, leaf.data, size);
  }
  const [root] = nodes;
  if (!root) throw new Error('no root entry');
  const miniStream = new Uint8Array(miniParts.length * MINI_SECTOR);
  miniParts.forEach((part, index) => {
    miniStream.set(part, index * MINI_SECTOR);
  });
  root.start = place(sectors, fat, miniStream, size);
  root.size = miniStream.length;
  const miniFatBytes = miniFat.length > 0 ? tableBytes(miniFat, size) : new Uint8Array(0);
  const miniFatStart = place(sectors, fat, miniFatBytes, size);
  const directoryBytes = directory(nodes, size);
  const directoryStart = place(sectors, fat, directoryBytes, size);
  const counts = tableCounts(sectors.length, size);
  const fatStart = sectors.length;
  sectors.push(...tableSectors(fat, size, fatStart, counts));
  const head = header(
    size,
    version,
    { ...counts, fatStart, directory: directoryBytes },
    {
      directory: directoryStart,
      miniFat: miniFatStart,
      miniFatSectors: miniFatBytes.length / size,
    },
  );
  const out = new Uint8Array(size * (sectors.length + 1));
  out.set(head);
  sectors.forEach((sector, index) => {
    out.set(sector, (index + 1) * size);
  });
  return out;
}
