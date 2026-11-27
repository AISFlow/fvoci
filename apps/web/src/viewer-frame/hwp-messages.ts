/**
 * What the HWP renderer (hwp.ts) and its worker (hwp-worker.ts) say to each
 * other. The worker answers each message with one: `open` with `opened` or
 * `failed`, `page` with `page` or `failed`.
 *
 * The editor's frame (src/editor-frame/main.ts) asks the same worker for the
 * check alone, before the editor reads a file: `check`, answered with
 * `checked` or `failed`. It opens a saved file in it as the viewer does, and
 * then asks how many controls of each kind it holds: `controls`, answered
 * with `controls` or `failed`. And it opens the file it edits, and asks for
 * the pages it lays out on once saved as rhwp saves it and read back:
 * `resaved`, answered with `resaved` or `failed`.
 */
import type { ControlTally } from './hwp-controls.ts';

export type HwpFormat = 'hwp' | 'hwpx';

/** A page's size in CSS pixels at the document's own size. */
export interface PageSize {
  width: number;
  height: number;
}

/** A run of a page's text where the page draws it: what a screen reader reads, and what is selected. */
export interface TextRun {
  text: string;
  /** Its box on the page, in CSS pixels. */
  x: number;
  y: number;
  width: number;
  height: number;
  /** Its font's size, in CSS pixels. */
  size: number;
}

export type ToWorker =
  /** The file, and rhwp's WebAssembly as the app fetched it ($lib/file-viewer/engine.ts). */
  | { kind: 'open'; format: HwpFormat; bytes: ArrayBuffer; wasm: ArrayBuffer }
  /** A page, from 1. */
  | { kind: 'page'; page: number }
  /** The file, to be checked as `open` checks it and handed back as rhwp may read it. */
  | { kind: 'check'; format: HwpFormat; bytes: ArrayBuffer }
  /** How many controls of each kind the document opened holds. */
  | { kind: 'controls' }
  /** The document opened, saved as rhwp saves it, checked and read back as `open` reads a file. */
  | { kind: 'resaved' };

export type FromWorker =
  | { kind: 'opened'; pages: PageSize[] }
  | { kind: 'page'; svg: string; text: TextRun[] }
  | { kind: 'checked'; bytes: ArrayBuffer }
  | { kind: 'controls'; controls: ControlTally }
  /** The pages the document opened lays out on, and its saved file read back. */
  | { kind: 'resaved'; pages: number; saved: number }
  | { kind: 'failed'; reason: 'locked' | 'unreadable' | 'too-large' | 'renderer' };
