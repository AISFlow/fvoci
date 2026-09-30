import JSZip from "jszip";
import { checkDocxPackage } from "./docx-limits.ts";

/**
 * Total inflated size of an HWPX package, checked before rhwp sees it. Half
 * the DOCX budget; it still admits the largest real XML part rhwp cites (a
 * 75 MB section, `MAX_XML_SIZE` notes) with room for the rest of the package.
 * It bounds package bytes, not rhwp's memory: dense section XML was measured
 * to retain about 8.8 times its inflated size in wasm, and a section repeated
 * in the spine is parsed once per reference. That memory is bounded by the
 * per-document worker (`hwp-client.ts`) — its open deadline, wasm32's 4 GiB
 * and termination — not by this budget.
 */
export const HWPX_MAX_EXPANDED_BYTES = 128 * 1024 * 1024;

/** HWPX part count cap (a real package has tens of parts). */
export const HWPX_MAX_ENTRIES = 10_000;

export type HwpBytesCheck =
  | { status: "ok"; bytes: Uint8Array }
  | { status: "tooLarge" }
  | { status: "invalid" };

/** rhwp treats only a local-file-header ZIP as an HWPX candidate (`detect_format`). */
function isZip(bytes: Uint8Array): boolean {
  return bytes[0] === 0x50 && bytes[1] === 0x4b && bytes[2] === 0x03 && bytes[3] === 0x04;
}

/**
 * Bytes to hand rhwp. rhwp 0.8.6 caps each HWPX part (256 MiB XML, 512 MiB
 * binary) but not their sum, and eagerly reads every `Scripts/*` part even
 * when nothing references it, so a small package can inflate without bound.
 *
 * An HWPX (ZIP) package is therefore measured with `checkDocxPackage` — every
 * logical part stream-inflated, stopping past `maxExpanded`, declared sizes
 * not trusted — and then re-written by JSZip from that same logical view.
 * rhwp parses the re-written package, so what it can inflate is exactly what
 * was measured: shadowed duplicate records, directory records carrying data,
 * parts declared empty and names JSZip normalises are not carried over, and
 * JSZip's reading of the archive cannot differ from the zip crate's. Deflated
 * parts keep their compressed bytes; stored parts are deflated, which costs at
 * most the downloaded size.
 *
 * The budget bounds package bytes, not what rhwp builds from them: a part
 * referenced many times (a repeated spine item) is parsed each time. Other
 * formats (HWP 5 CFB, HWP 3, HML) are passed through; for HWP 5 rhwp bounds
 * decompressed streams at 256 MiB each and 512 MiB in total. Both rest on the
 * per-document worker (`hwp-client.ts`): its memory is released when the
 * viewer terminates it, and a parse that runs too long is terminated.
 */
export async function prepareHwpBytes(
  bytes: Uint8Array,
  isAlive: () => boolean,
  maxExpanded: number = HWPX_MAX_EXPANDED_BYTES,
  maxEntries: number = HWPX_MAX_ENTRIES,
): Promise<HwpBytesCheck> {
  if (!isZip(bytes)) return { status: "ok", bytes };
  const check = await checkDocxPackage(bytes, isAlive, maxExpanded, maxEntries);
  if (check !== "ok") return { status: check };
  try {
    const zip = await JSZip.loadAsync(bytes, { checkCRC32: false });
    if (!isAlive()) return { status: "invalid" };
    const rewritten = await zip.generateAsync({ type: "uint8array", compression: "DEFLATE" });
    return isAlive() ? { status: "ok", bytes: rewritten } : { status: "invalid" };
  } catch {
    return { status: "invalid" };
  }
}
