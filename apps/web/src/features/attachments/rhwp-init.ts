import init from "@rhwp/core";
import wasmUrl from "@rhwp/core/rhwp_bg.wasm?url";

type MeasureTextWidth = (font: string, text: string) => number;

/**
 * `@rhwp/core` README contract: register `globalThis.measureTextWidth` (canvas
 * `measureText`) before the first layout. 0.8.6 lays out SVG with its own
 * embedded font metrics and does not call the hook; it is kept because the
 * package still documents it as required. rhwp runs in a worker, which has
 * no `document`, so the hook measures on an `OffscreenCanvas`.
 */
function installMeasureTextWidth(): void {
  const g = globalThis as typeof globalThis & { measureTextWidth?: MeasureTextWidth };
  if (g.measureTextWidth || typeof OffscreenCanvas === "undefined") return;
  let ctx: OffscreenCanvasRenderingContext2D | null = null;
  let lastFont = "";
  g.measureTextWidth = (font, text) => {
    ctx ??= new OffscreenCanvas(1, 1).getContext("2d");
    if (!ctx) return 0;
    if (font !== lastFont) {
      ctx.font = font;
      lastFont = font;
    }
    return ctx.measureText(text).width;
  };
}

let compiled: Promise<WebAssembly.Module> | null = null;

/**
 * Compiles the rhwp WASM once per page (main thread). The module is a
 * same-origin build asset; its bytes are fetched and compiled (source
 * `rhwp-init.ts`), so this needs only `'wasm-unsafe-eval'`. Only the compiled
 * code is cached: every document instantiates it in its own worker, whose
 * memory goes away with the worker. A failed attempt is not cached, so a
 * retry fetches again.
 */
export function loadRhwpModule(): Promise<WebAssembly.Module> {
  compiled ??= (async () => {
    const response = await fetch(wasmUrl, { credentials: "same-origin" });
    if (!response.ok) {
      await response.body?.cancel();
      throw new Error(`rhwp wasm ${response.status}`);
    }
    return WebAssembly.compile(await response.arrayBuffer());
  })();
  compiled.catch(() => {
    compiled = null;
  });
  return compiled;
}

/** Instantiates rhwp inside a document worker from the page's compiled module. */
export async function initRhwp(module: WebAssembly.Module): Promise<void> {
  installMeasureTextWidth();
  await init({ module_or_path: module });
}
