// Source apps/web/src/features/settings/settings-account-mfa.tsx `QrSvg`
// geometry, split out so it can be unit tested without a DOM.
import qrcode from "qrcode-generator";

export type QrModules = { size: number; path: string };

/**
 * Error correction `M`, automatic version (source `qrcode(0, "M")`). Dark
 * modules become one SVG path of unit squares; the caller renders it as a
 * React element, never as library-built SVG markup (no innerHTML).
 */
export function qrModules(text: string): QrModules {
  const qr = qrcode(0, "M");
  qr.addData(text);
  qr.make();
  const size = qr.getModuleCount();
  let path = "";
  for (let row = 0; row < size; row++) {
    for (let col = 0; col < size; col++) {
      if (qr.isDark(row, col)) path += `M${col} ${row}h1v1h-1z`;
    }
  }
  return { size, path };
}
