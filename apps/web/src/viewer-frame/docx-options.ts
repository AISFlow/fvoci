/**
 * How docx-preview is asked to draw a Word document, in the worker
 * (`docx-draw.ts`) and, in development, in the frame (`docx.ts`). A module
 * of its own, so that the frame's chunk takes these without the library.
 */
import type { Options } from 'docx-preview';

export const OPTIONS = {
  className: 'docx',
  inWrapper: true,
  breakPages: true,
  // Pages break where Word last laid the document out, as well as where it says so:
  // otherwise a document without manual breaks is one long page.
  ignoreLastRenderedPageBreak: false,
  // Fonts are read as bytes the library changes in place, so they cannot be unpacked once
  // for all; the app's fonts stand in for them.
  ignoreFonts: true,
  // Its pattern for the declaration takes time that grows with the square of a line.
  trimXmlDeclaration: false,
  renderAltChunks: false,
  renderComments: false,
  renderChanges: false,
  useBase64URL: false,
  experimental: false,
} satisfies Partial<Options>;
