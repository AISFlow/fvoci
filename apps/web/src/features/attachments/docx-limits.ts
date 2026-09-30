import JSZip from "jszip";

// Defined beside the other caps so the attachment page can read it without this module's zip reader.
export { DOCX_MAX_BYTES } from "./viewer-download.ts";

/** Total inflated size of every package part, checked before the renderer runs. */
export const DOCX_MAX_EXPANDED_BYTES = 256 * 1024 * 1024;

/** Package part count cap (a real document has tens of parts). */
export const DOCX_MAX_ENTRIES = 10_000;

export type DocxPackageCheck = "ok" | "tooLarge" | "invalid";

type ByteStream = {
  on(event: "data", callback: (chunk: Uint8Array) => void): ByteStream;
  on(event: "end", callback: () => void): ByteStream;
  on(event: "error", callback: (error: unknown) => void): ByteStream;
  resume(): ByteStream;
  pause(): ByteStream;
};

/**
 * docx-preview inflates whole parts in memory with no size bound, so a small
 * ZIP could expand without limit. Checked with JSZip (the renderer's own ZIP
 * reader) over the same logical view the renderer builds:
 *
 * 1. central directory only (`checkCRC32: false` inflates nothing);
 * 2. part count, then every part stream-inflated one at a time, stopping at
 *    the first chunk past `maxExpanded` — declared sizes are not trusted.
 *    JSZip inflates one 16 KiB compressed block synchronously, so the stop can
 *    overshoot by that block's output (≤ ~16 MiB at DEFLATE's ~1032:1 limit).
 *
 * The caps apply to the logical parts in `zip.files` (last name wins after
 * JSZip's path normalisation, directories empty) — exactly what
 * docx-preview's own `loadAsync` (defaults, no CRC) can inflate. Shadowed
 * duplicate, colliding or directory records are never inflated by either
 * load, so they are neither counted nor rendered. There is deliberately no
 * `checkCRC32: true` load: it inflates every raw record, including those
 * shadowed ones, outside this budget. A CRC mismatch alone is therefore not a
 * failure here; CRC is corruption detection, not authentication, and the
 * bytes are the authenticated download itself. Malformed ZIP or DEFLATE data
 * still fails the check, and malformed XML fails the render.
 *
 * The caps bound inflated bytes and part count, not a universal browser CPU
 * or latency limit for parsing and layout.
 *
 * Cancellation (`isAlive() === false`) is honoured before any inflation and
 * between parts and chunks.
 */
export async function checkDocxPackage(
  bytes: Uint8Array,
  isAlive: () => boolean,
  maxExpanded: number = DOCX_MAX_EXPANDED_BYTES,
  maxEntries: number = DOCX_MAX_ENTRIES,
): Promise<DocxPackageCheck> {
  if (!isAlive()) return "invalid";
  let zip: JSZip;
  try {
    zip = await JSZip.loadAsync(bytes, { checkCRC32: false });
  } catch {
    return "invalid";
  }
  if (!isAlive()) return "invalid";
  const parts = Object.values(zip.files).filter((entry) => !entry.dir);
  if (parts.length > maxEntries) return "tooLarge";
  let total = 0;
  for (const part of parts) {
    if (!isAlive()) return "invalid";
    const internal = (
      part as unknown as { internalStream(type: "uint8array"): ByteStream }
    ).internalStream("uint8array");
    const result = await new Promise<DocxPackageCheck>((resolve) => {
      internal
        .on("data", (chunk) => {
          total += chunk.byteLength;
          if (total > maxExpanded || !isAlive()) {
            internal.pause();
            resolve(total > maxExpanded ? "tooLarge" : "invalid");
          }
        })
        .on("error", () => resolve("invalid"))
        .on("end", () => resolve("ok"))
        .resume();
    });
    if (result !== "ok") return result;
  }
  return isAlive() ? "ok" : "invalid";
}
