/** The two formats rhwp writes. */
export type HwpExportFormat = "hwp" | "hwpx";

/**
 * The format an edited copy is written in (source `exportDocument` /
 * `mimeForName`): taken from the original file name, not its MIME type, so
 * `x.hwpx` stays HWPX (OWPML) and anything else becomes HWP 5.0.
 */
export function hwpExportFormat(name: string): { format: HwpExportFormat; mime: string } {
  return /\.hwpx$/i.test(name)
    ? { format: "hwpx", mime: "application/hwpx" }
    : { format: "hwp", mime: "application/x-hwp" };
}
