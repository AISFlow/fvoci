/**
 * Keeps the renderer's placeholder labels inside their own shape.
 *
 * `@office-kit/pptx-preview` draws a picture it has no bytes for (a linked
 * `r:link` picture, a missing or undecodable embed), a chart kind it does not
 * model and an unrecognised graphic frame as
 * `<g data-pptx-fallback="image|chart|graphicFrame" transform?><rect x y
 * width height …/><title/><text …>label</text>…</g>`. The label is centred
 * on the box, unclipped, in 13 pt bold sans-serif: "picture (link: …)" runs
 * hundreds of px past a small box and paints over whatever lies beside it.
 *
 * Each such label is wrapped, as is, in a nested `<svg>` viewport of exactly
 * the rect's box, whose `viewBox` is that same box: the label keeps its
 * coordinates, and anything outside the box is cut (`overflow="hidden"`).
 * The viewport is in the group's user space, as the rect is, so the group's
 * rotation or flip applies to both. A box without area shows no label.
 *
 * The markup is read with a standard XML tokenizer (saxes) only to find
 * those groups; the only change to it is the two inserted tags per label.
 * This is not a sanitizer: the slide stays an image inside an image
 * (`pptx-svg.ts`). Markup that is not well-formed XML, or a fallback group of
 * another shape than the one above, is `failed` — the slide is then not
 * shown, as an image parse of it would not be either.
 */

import { SaxesParser, type SaxesTagNS } from "saxes";

const SVG_NS = "http://www.w3.org/2000/svg";
const LABELLED = new Set(["image", "chart", "graphicFrame"]);
const MARKER = "data-pptx-fallback";
/** A renderer coordinate (`E()` writes fixed 2-decimal px). */
const NUMBER = /^-?[0-9]+(?:\.[0-9]+)?$/;

export type BoundedLabels = { status: "ok"; svg: string } | { status: "failed" };

type Box = { x: number; y: number; width: number; height: number };

type Frame = {
  /** Set on a labelled fallback group: how far its children have been read. */
  fallback?: { box: Box | null; label: "pending" | "open" | "done" };
  /** The label element of the enclosing fallback group. */
  label?: true;
};

class Unexpected extends Error {}

function number(tag: SaxesTagNS, name: string): number {
  const value = tag.attributes[name]?.value ?? "";
  const parsed = Number(value);
  if (!(NUMBER.test(value) && Number.isFinite(parsed))) throw new Unexpected(`${name}=${value.slice(0, 32)}`);
  return parsed;
}

function viewport(box: Box): string {
  if (!(box.width > 0 && box.height > 0)) return '<svg width="0" height="0">';
  const { x, y, width, height } = box;
  return `<svg x="${x}" y="${y}" width="${width}" height="${height}" viewBox="${x} ${y} ${width} ${height}" overflow="hidden">`;
}

/** `svg` with every renderer placeholder label clipped to its placeholder box. */
export function boundFallbackLabels(svg: string): BoundedLabels {
  // Nothing to find: no parse (slides carry up to tens of MiB of inlined pictures).
  if (!svg.includes(MARKER)) return { status: "ok", svg };
  const inserts: { at: number; text: string }[] = [];
  const stack: Frame[] = [];
  const parser = new SaxesParser({ xmlns: true, position: true });
  parser.on("error", (error) => {
    throw error;
  });
  parser.on("opentag", (tag) => {
    const parent = stack[stack.length - 1]?.fallback;
    const svgTag = tag.uri === SVG_NS;
    let label = false;
    if (parent && parent.label === "pending") {
      if (parent.box === null) {
        // The first child is the placeholder box.
        if (!(svgTag && tag.local === "rect")) throw new Unexpected(`first child ${tag.name}`);
        parent.box = {
          x: number(tag, "x"),
          y: number(tag, "y"),
          width: number(tag, "width"),
          height: number(tag, "height"),
        };
      } else if (svgTag && tag.local === "text") {
        parent.label = "open";
        label = true;
        inserts.push({ at: svg.lastIndexOf("<", parser.position - 1), text: viewport(parent.box) });
      } else if (!(svgTag && tag.local === "title")) {
        throw new Unexpected(`child ${tag.name} before the label`);
      }
    }
    const kind = tag.attributes[MARKER];
    const labelled = svgTag && tag.local === "g" && kind !== undefined && kind.uri === "" && LABELLED.has(kind.value);
    stack.push({
      ...(labelled ? { fallback: { box: null, label: "pending" as const } } : {}),
      ...(label ? { label: true as const } : {}),
    });
  });
  parser.on("closetag", () => {
    const frame = stack.pop()!;
    if (frame.fallback && frame.fallback.label !== "done") throw new Unexpected("fallback without a label");
    if (frame.label) {
      stack[stack.length - 1]!.fallback!.label = "done";
      inserts.push({ at: parser.position, text: "</svg>" });
    }
  });
  try {
    parser.write(svg).close();
  } catch {
    return { status: "failed" };
  }
  if (inserts.length === 0) return { status: "ok", svg };
  const parts: string[] = [];
  let at = 0;
  for (const insert of inserts) {
    parts.push(svg.slice(at, insert.at), insert.text);
    at = insert.at;
  }
  parts.push(svg.slice(at));
  return { status: "ok", svg: parts.join("") };
}
