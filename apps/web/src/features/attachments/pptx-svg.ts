import { isSafeDataImageUrl } from "./docx-frame.ts";

/**
 * Hardening for `@office-kit/pptx-preview` slide SVG before it is shown.
 *
 * The slide is only ever displayed through `<img>` from a blob URL, where a
 * browser runs no script, follows no link and loads no external resource.
 * That is the isolation boundary. On top of it, the served markup itself
 * carries no navigation or remote reference, so it stays inert even if the
 * blob is opened as a document: the renderer emits `<a href>` (with
 * `target="_blank"`) for run and shape hyperlinks, including `javascript:`
 * targets taken verbatim from the deck, and puts deck font names into CSS.
 *
 * This works on the markup string, not a DOM: every in-page DOM parse
 * (DOMParser, an inert HTML document, `<template>`) inherits the app CSP and
 * reports each `style` attribute as a `style-src` violation. It is exact for
 * this renderer's output, which escapes `& < > "` in all text and attribute
 * values, so a tag is precisely `<[^<>]*>` and a quoted value `"[^"]*"`.
 * Anything outside that shape or the renderer's element set — declarations,
 * processing instructions, comments, script, SMIL, `<style>` or an event
 * attribute — makes the slide unavailable rather than being rewritten.
 */

const TAG = /<[^<>]*>/g;

const DENIED_TAG =
  /^<\/?(script|iframe|frame|frameset|object|embed|applet|link|meta|base|form|input|button|textarea|select|template|portal|noscript|audio|video|source|track|style|animate|animatemotion|animatetransform|set|discard|handler|listener|feimage)\b/i;

/** Declarations, processing instructions, comments, CDATA: never emitted by the renderer. */
const DECLARATION = /^<[!?]/;

const EVENT_ATTRIBUTE = /\son[a-z]+\s*=/i;

const ANCHOR = /^<\/?a(\s|>)/i;

const URL_ATTRIBUTE = /(\s)((?:[a-z]+:)?href|src)\s*=\s*"([^"]*)"/gi;

const SAFE_DATA_FONT = /^data:(font\/|application\/(font|x-font|octet-stream|vnd\.ms-))/i;

/** A same-document reference such as `#clip-3`. */
function isFragment(url: string): boolean {
  return /^#\S*$/.test(url.trim());
}

/**
 * Replaces every CSS or presentation-attribute `url(...)` that is not a
 * same-document fragment or an embedded image/font with `none`, and drops
 * `@import`.
 */
export function neutralizeSvgUrls(value: string): string {
  return value
    .replace(/@import[^;]*;?/gi, "")
    .replace(/url\(\s*(?:"([^"]*)"|'([^']*)'|([^)]*))\s*\)/gi, (whole, dq, sq, bare) => {
      const target = String(dq ?? sq ?? bare ?? "").trim();
      return isFragment(target) || isSafeDataImageUrl(target) || SAFE_DATA_FONT.test(target) ? whole : "none";
    });
}

/** Undoes the renderer's attribute escaping, for checking a URL value. */
function unescapeAttribute(value: string): string {
  return value
    .replace(/&quot;/g, '"')
    .replace(/&apos;/g, "'")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&amp;/g, "&");
}

/**
 * Returns the slide SVG with navigation and remote references removed, or
 * `null` when it holds markup the renderer never produces. `<a>` tags are
 * unwrapped (link text and styling stay); `href`/`src` values other than
 * fragments and embedded `data:` images are dropped; CSS `url()` to anything
 * else becomes `none`. Text content is never touched.
 */
export function sanitizeSlideSvg(svg: string): string | null {
  if (!/^<svg[\s>]/.test(svg) || !/<\/svg>\s*$/.test(svg)) return null;
  let rejected = false;
  const out = svg.replace(TAG, (tag) => {
    if (rejected) return tag;
    // Attribute names only: quoted values (e.g. a deck font name) cannot trip the check.
    const names = tag.replace(/"[^"]*"/g, '""');
    if (DECLARATION.test(tag) || DENIED_TAG.test(tag) || EVENT_ATTRIBUTE.test(names)) {
      rejected = true;
      return tag;
    }
    if (ANCHOR.test(tag)) return "";
    let clean = tag.replace(URL_ATTRIBUTE, (whole, space: string, _name: string, value: string) => {
      const url = unescapeAttribute(value);
      return isFragment(url) || isSafeDataImageUrl(url) ? whole : space.trimEnd();
    });
    if (/url\(|@import/i.test(clean)) clean = neutralizeSvgUrls(clean);
    return clean;
  });
  return rejected ? null : out;
}
