import { getSlides, getSlideSize, loadPresentation, type PresentationData, type SlideData } from "@office-kit/pptx";
import { renderSlideToSvg } from "@office-kit/pptx-preview";
import { PPTX_MAX_SLIDE_SVG_BYTES, PPTX_MAX_SLIDES, repackPptx } from "./pptx-limits.ts";
import { slideImageSvg, type SlideImageSvg } from "./pptx-svg.ts";

const EMU_PER_PX = 9525;

/** 16:9 at 960 × 540 px, the renderer's own default when a deck omits `p:sldSz`. */
const DEFAULT_SIZE = { width: 960, height: 540 };

export type PptxDeck = {
  pres: PresentationData;
  slides: readonly SlideData[];
  /** Slide size in CSS px at 100% zoom. */
  width: number;
  height: number;
};

export type PptxOpen = { status: "ok"; deck: PptxDeck } | { status: "tooLarge" } | { status: "invalid" };

export type PptxLimits = { maxSlides: number; maxSlideSvgBytes: number };

export const PPTX_LIMITS: PptxLimits = {
  maxSlides: PPTX_MAX_SLIDES,
  maxSlideSvgBytes: PPTX_MAX_SLIDE_SVG_BYTES,
};

/**
 * Opens the original PPTX bytes (source `PptxViewer`: `loadPresentation` +
 * `getSlides`) after the bounded package check in `repackPptx`. A deck with
 * no slide, or more than `maxSlides`, is not shown.
 */
export async function openPptx(
  bytes: Uint8Array,
  isAlive: () => boolean,
  limits: PptxLimits = PPTX_LIMITS,
): Promise<PptxOpen> {
  const repacked = await repackPptx(bytes, isAlive);
  if (repacked.status !== "ok") return repacked;
  if (!isAlive()) return { status: "invalid" };
  let pres: PresentationData;
  let slides: readonly SlideData[];
  try {
    pres = await loadPresentation(repacked.bytes);
    slides = getSlides(pres);
  } catch {
    return { status: "invalid" };
  }
  if (slides.length === 0) return { status: "invalid" };
  if (slides.length > limits.maxSlides) return { status: "tooLarge" };
  const size = getSlideSize(pres);
  const width = size ? size.width / EMU_PER_PX : DEFAULT_SIZE.width;
  const height = size ? size.height / EMU_PER_PX : DEFAULT_SIZE.height;
  if (!(width > 0 && height > 0 && Number.isFinite(width) && Number.isFinite(height))) return { status: "invalid" };
  return { status: "ok", deck: { pres, slides, width, height } };
}

export type SlideSvg = { status: "ok"; svg: string } | { status: "tooLarge" } | { status: "failed" };

/**
 * One slide as the renderer's SVG markup (source `renderSlideToSvg`, browser
 * entry). Output that cannot be `maxSlideSvgBytes` of UTF-8 — pictures are
 * inlined as base64 — is not shown. The layout is synchronous and can be
 * slow (super-linear in a text box's paragraph count), so the viewer runs it
 * in a worker (`pptx-worker.ts`) with a wall-clock bound.
 */
export function renderSlide(deck: PptxDeck, index: number, limits: PptxLimits = PPTX_LIMITS): SlideSvg {
  const slide = deck.slides[index];
  if (!slide) return { status: "failed" };
  let svg: string;
  try {
    svg = renderSlideToSvg(deck.pres, slide);
  } catch {
    return { status: "failed" };
  }
  if (svg.length > limits.maxSlideSvgBytes) return { status: "tooLarge" };
  return { status: "ok", svg };
}

/**
 * One slide as the page may show it: the renderer's SVG wrapped as an image
 * in the fixed outer template of `pptx-svg.ts`.
 */
export function renderSlideImage(deck: PptxDeck, index: number, limits: PptxLimits = PPTX_LIMITS): SlideImageSvg {
  const rendered = renderSlide(deck, index, limits);
  if (rendered.status !== "ok") return rendered;
  return slideImageSvg(rendered.svg, deck.width, deck.height, limits.maxSlideSvgBytes);
}
