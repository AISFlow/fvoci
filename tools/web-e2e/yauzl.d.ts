// The part of yauzl 3.4.0's public API that trace-summary.ts uses
// (https://github.com/thejoshwolfe/yauzl/blob/3.4.0/README.md).
declare module "yauzl" {
  import type { Readable } from "node:stream";

  export interface ExtraField {
    id: number;
    data: Buffer;
  }

  export interface Entry {
    versionMadeBy: number;
    versionNeededToExtract: number;
    generalPurposeBitFlag: number;
    compressionMethod: number;
    crc32: number;
    compressedSize: number;
    uncompressedSize: number;
    fileNameLength: number;
    extraFieldLength: number;
    fileCommentLength: number;
    externalFileAttributes: number;
    relativeOffsetOfLocalHeader: number;
    fileNameRaw: Buffer;
    extraFields: ExtraField[];
  }

  export interface LocalFileHeader {
    fileDataStart: number;
    generalPurposeBitFlag: number;
    fileName: Buffer;
  }

  export interface ZipFile {
    entryCount: number;
    eachEntry(): AsyncIterable<Entry>;
    readLocalFileHeaderPromise(entry: Entry): Promise<LocalFileHeader>;
    openReadStreamPromise(entry: Entry): Promise<Readable>;
    close(): void;
  }

  export interface Options {
    lazyEntries?: boolean;
    decodeStrings?: boolean;
    validateEntrySizes?: boolean;
    strictFileNames?: boolean;
  }

  export function fromBufferPromise(buffer: Buffer, options?: Options): Promise<ZipFile>;
  export function getFileNameLowLevel(
    generalPurposeBitFlag: number,
    fileNameBuffer: Buffer,
    extraFields: ExtraField[],
    strictFileNames: boolean,
  ): string;

  const yauzl: {
    fromBufferPromise: typeof fromBufferPromise;
    getFileNameLowLevel: typeof getFileNameLowLevel;
  };
  export default yauzl;
}
