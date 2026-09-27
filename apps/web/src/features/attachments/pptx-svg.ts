import { isSafeDataImageUrl } from "./docx-frame.ts";

/**
 * Hardening for `@office-kit/pptx-preview` slide SVG before it is shown.
 *
 * The slide is only ever displayed through `<img>` from a blob URL, where a
 * browser runs no script, follows no link and loads no external resource. On
 * top of that, the markup itself is made inert here, so the served image
 * carries no navigation or remote reference at all: the renderer emits
 * `<a href>` (with `target="_blank"`) for run and shape hyperlinks, including
 * `javascript:` targets taken verbatim from the deck, and its CSS can carry
 * deck-supplied font names.
 */

const DENIED_ELEMENTS = new Set([
  "script",
  "iframe",
  "frame",
  "frameset",
  "object",
  "embed",
  "applet",
  "link",
  "meta",
  "base",
  "form",
  "input",
  "button",
  "textarea",
  "select",
  "template",
  "portal",
  "noscript",
  "audio",
  "video",
  "source",
  "track",
  // SMIL can rewrite attributes (such as `href`) after sanitizing.
  "animate",
  "animatemotion",
  "animatetransform",
  "set",
  "discard",
]);

const URL_ATTRIBUTES = new Set(["href", "xlink:href", "src", "srcset", "poster", "background", "data", "action", "formaction"]);

const SAFE_DATA_FONT = /^data:(font\/|application\/(font|x-font|octet-stream|vnd\.ms-))/i;

/** A same-document reference such as `#clip-3`. */
function isFragment(url: string): boolean {
  return /^#[^\s]*$/.test(url.trim());
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

function unwrap(element: Element): void {
  const parent = element.parentNode;
  if (!parent) return;
  while (element.firstChild) parent.insertBefore(element.firstChild, element);
  element.remove();
}

/**
 * Returns the slide SVG with active and navigational content removed, or
 * `null` when it does not parse as one SVG document. Links keep their text
 * and styling but lose their targets (`<a>` is unwrapped); only fragment and
 * embedded `data:` image references survive.
 */
export function sanitizeSlideSvg(svg: string): string | null {
  const parsed = new DOMParser().parseFromString(svg, "image/svg+xml");
  const root = parsed.documentElement;
  if (root.localName !== "svg" || parsed.getElementsByTagName("parsererror").length > 0) return null;

  const all = [...root.getElementsByTagName("*")];
  for (const element of all) {
    const name = element.localName.toLowerCase();
    if (DENIED_ELEMENTS.has(name)) element.remove();
  }
  for (const anchor of [...root.getElementsByTagName("*")].filter((el) => el.localName.toLowerCase() === "a")) {
    unwrap(anchor);
  }
  for (const element of [root, ...root.getElementsByTagName("*")]) {
    for (const attr of [...element.attributes]) {
      const name = attr.name.toLowerCase();
      if (name.startsWith("on")) {
        element.removeAttributeNode(attr);
      } else if (URL_ATTRIBUTES.has(name) || name.endsWith(":href")) {
        if (!isFragment(attr.value) && !isSafeDataImageUrl(attr.value)) element.removeAttributeNode(attr);
      } else if (/url\(|@import/i.test(attr.value)) {
        element.setAttribute(attr.name, neutralizeSvgUrls(attr.value));
      }
    }
  }
  for (const style of [...root.getElementsByTagName("*")].filter((el) => el.localName.toLowerCase() === "style")) {
    style.textContent = neutralizeSvgUrls(style.textContent ?? "");
  }
  return new XMLSerializer().serializeToString(parsed);
}
