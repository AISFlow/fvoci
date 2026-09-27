/**
 * Isolation for docx-preview output (source `isolated-frame.ts` contract).
 *
 * The renderer builds DOM from untrusted OOXML. It runs against an inert
 * scratch document (no browsing context: nothing loads, nothing runs), the
 * result is sanitized, then copied into a sandboxed iframe without
 * `allow-scripts` whose CSP forbids every fetch except `data:` images and
 * fonts. A `srcdoc` frame also inherits the app CSP (`style-src 'self'`), and
 * policies only add up, so document CSS goes in as constructed stylesheets
 * and inline styles through the CSSOM — neither is a CSP-checked inline style.
 */

export const DOCX_FRAME_CSP =
  "default-src 'none'; script-src 'none'; style-src 'none'; img-src data:; font-src data:; base-uri 'none'; form-action 'none'";

const FRAME_HTML = `<!DOCTYPE html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="${DOCX_FRAME_CSP}"></head><body></body></html>`;

/** Base page chrome inside the frame; fonts fall back to the viewer's system CJK/emoji fonts. */
export const DOCX_FRAME_BASE_CSS =
  "html,body{margin:0;background:transparent}body{font-family:sans-serif}.docx-wrapper{background:transparent!important;padding:16px!important}.docx-wrapper>section.docx{margin:0 auto!important;box-shadow:0 1px 3px rgba(0,0,0,.18)}";

const DENIED_ELEMENTS =
  "script,iframe,frame,frameset,object,embed,applet,link,meta,base,form,input,button,textarea,select,template,portal,noscript";

const URL_ATTRIBUTES = new Set(["src", "href", "xlink:href", "srcset", "poster", "background", "data"]);

const SAFE_DATA_IMAGE = /^data:image\/(png|jpeg|gif|webp|bmp|x-emf|x-wmf|tiff|svg\+xml)[;,]/i;
const SAFE_DATA_FONT = /^data:(font\/|application\/(font|x-font|octet-stream|vnd\.ms-))/i;

export function isSafeDataImageUrl(url: string): boolean {
  return SAFE_DATA_IMAGE.test(url.trim());
}

/**
 * Replaces every CSS `url(...)` that is not an embedded image or font with
 * `none`, and drops `@import`. The frame CSP blocks such fetches anyway; this
 * keeps them out of the document instead of relying on reports.
 */
export function neutralizeCssUrls(css: string): string {
  return css
    .replace(/@import[^;]*;?/gi, "")
    .replace(/url\(\s*(?:"([^"]*)"|'([^']*)'|([^)]*))\s*\)/gi, (whole, dq, sq, bare) => {
      const target = String(dq ?? sq ?? bare ?? "").trim();
      return SAFE_DATA_IMAGE.test(target) || SAFE_DATA_FONT.test(target) ? whole : "none";
    });
}

/** Mirrors docx-preview's default element factory, but creates nodes in `scratch`. */
export function inertElementFactory(scratch: Document) {
  type HElement = {
    ns?: string;
    tagName: string;
    className?: string;
    style?: Record<string, string> | string;
    children?: (HElement | Node | string)[];
  } & Record<string, unknown>;
  const h = (elem: HElement | Node | string): Node => {
    if (typeof elem === "string") return scratch.createTextNode(elem);
    if (elem instanceof Node) return elem;
    const { ns, tagName, className, style, children, ...props } = elem;
    if (tagName === "#fragment") {
      const fragment = scratch.createDocumentFragment();
      for (const child of children ?? []) fragment.appendChild(h(child));
      return fragment;
    }
    if (tagName === "#comment") {
      return scratch.createComment(typeof children?.[0] === "string" ? children[0] : "");
    }
    const result = ns ? scratch.createElementNS(ns, tagName) : scratch.createElement(tagName);
    if (className) result.setAttribute("class", className);
    if (typeof style === "string") {
      result.setAttribute("style", style);
    } else if (style) {
      Object.assign((result as HTMLElement).style, style);
    }
    for (const [key, value] of Object.entries(props)) {
      if (value !== undefined) (result as unknown as Record<string, unknown>)[key] = value;
    }
    for (const child of children ?? []) result.appendChild(h(child));
    return result;
  };
  return h;
}

/**
 * Strips active content and navigation from rendered DOCX markup: denied
 * elements, event handlers, every link target (source
 * `stripExternalNavigation`) and any image/resource URL that is not an
 * embedded `data:` image. Link text and styling stay.
 */
export function sanitizeRenderedDocx(root: Element | DocumentFragment): void {
  for (const node of root.querySelectorAll(DENIED_ELEMENTS)) node.remove();
  for (const style of root.querySelectorAll("style")) {
    style.textContent = neutralizeCssUrls(style.textContent ?? "");
  }
  for (const el of root.querySelectorAll("*")) {
    for (const attr of [...el.attributes]) {
      const name = attr.name.toLowerCase();
      if (name.startsWith("on") || name === "srcdoc" || name === "formaction" || name === "action") {
        el.removeAttributeNode(attr);
      } else if (el.localName === "a" && (name === "href" || name === "xlink:href" || name === "target")) {
        el.removeAttributeNode(attr);
      } else if (URL_ATTRIBUTES.has(name) && !isSafeDataImageUrl(attr.value)) {
        el.removeAttributeNode(attr);
      } else if (name === "style" && /url\(|expression\(|@import/i.test(attr.value)) {
        el.setAttribute("style", neutralizeCssUrls(attr.value));
      }
    }
  }
}

/** Adopts `cssTexts` into the frame document as constructed stylesheets of the frame's realm. */
export function adoptFrameStyles(doc: Document, win: Window, cssTexts: string[]): void {
  const Sheet = (win as Window & { CSSStyleSheet: typeof CSSStyleSheet }).CSSStyleSheet;
  const sheets = cssTexts
    .map((text) => text.trim())
    .filter((text) => text.length > 0)
    .map((text) => {
      const sheet = new Sheet();
      sheet.replaceSync(neutralizeCssUrls(text));
      return sheet;
    });
  doc.adoptedStyleSheets = [...doc.adoptedStyleSheets, ...sheets];
}

/**
 * Re-applies each element's inline style through the CSSOM after
 * `importNode` (source `transferInlineStyles`); `source` and `destination`
 * are the same tree.
 */
export function transferInlineStyles(source: Element, destination: Element): void {
  const from = source as HTMLElement;
  const to = destination as HTMLElement;
  if (from.style && to.style) {
    const text = from.style.cssText;
    to.removeAttribute("style");
    if (text) to.style.cssText = text;
  }
  const sourceKids = source.children;
  const destKids = destination.children;
  for (let i = 0; i < sourceKids.length && i < destKids.length; i += 1) {
    transferInlineStyles(sourceKids[i]!, destKids[i]!);
  }
}

function hasCspMeta(doc: Document | null): boolean {
  return Boolean(doc?.querySelector('meta[http-equiv="Content-Security-Policy"]'));
}

/**
 * Creates the sandboxed frame. `ready` settles once the `srcdoc` document (with
 * its CSP meta) has loaded, not on the initial about:blank.
 */
export function createDocxFrame(doc: Document): { frame: HTMLIFrameElement; ready: Promise<void> } {
  const frame = doc.createElement("iframe");
  frame.title = "DOCX";
  frame.setAttribute("sandbox", "allow-same-origin");
  frame.setAttribute("referrerpolicy", "no-referrer");
  frame.className = "attachment-viewer__docx-frame";
  const ready = new Promise<void>((resolve) => {
    const done = () => {
      if (hasCspMeta(frame.contentDocument)) {
        frame.removeEventListener("load", done);
        resolve();
      }
    };
    frame.addEventListener("load", done);
  });
  frame.srcdoc = FRAME_HTML;
  return { frame, ready };
}
