import JSZip from "jszip";

/** Largest DOCX the browser viewer downloads; bigger files stay download-only. */
export const DOCX_MAX_BYTES = 32 * 1024 * 1024;

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
 * ZIP could expand without limit. Stream-inflate every part with JSZip (the
 * renderer's own ZIP reader) and stop at the first byte past `maxExpanded`.
 * Declared sizes in the ZIP headers are not trusted.
 */
export async function checkDocxPackage(
  bytes: Uint8Array,
  isAlive: () => boolean,
  maxExpanded: number = DOCX_MAX_EXPANDED_BYTES,
  maxEntries: number = DOCX_MAX_ENTRIES,
): Promise<DocxPackageCheck> {
  let zip: JSZip;
  try {
    zip = await JSZip.loadAsync(bytes, { checkCRC32: true });
  } catch {
    return "invalid";
  }
  const parts = Object.values(zip.files).filter((entry) => !entry.dir);
  if (parts.length > maxEntries) return "tooLarge";
  let total = 0;
  for (const part of parts) {
    if (!isAlive()) return "invalid";
    const internal = (part as unknown as { internalStream(type: "uint8array"): ByteStream })
      .internalStream("uint8array");
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
  return "ok";
}
