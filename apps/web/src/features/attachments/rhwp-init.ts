import init from "@rhwp/core";
import wasmUrl from "@rhwp/core/rhwp_bg.wasm?url";

type MeasureTextWidth = (font: string, text: string) => number;

/**
 * `@rhwp/core` README contract: register `globalThis.measureTextWidth` (canvas
 * `measureText`) before the first layout. 0.8.6 also measures through its own
 * canvas binding; the hook is kept because the package still documents it as
 * required.
 */
function installMeasureTextWidth(): void {
  const g = globalThis as typeof globalThis & { measureTextWidth?: MeasureTextWidth };
  if (g.measureTextWidth) return;
  let ctx: CanvasRenderingContext2D | null = null;
  let lastFont = "";
  g.measureTextWidth = (font, text) => {
    ctx ??= document.createElement("canvas").getContext("2d");
    if (!ctx) return 0;
    if (font !== lastFont) {
      ctx.font = font;
      lastFont = font;
    }
    return ctx.measureText(text).width;
  };
}

let ready: Promise<void> | null = null;

/**
 * Instantiates the rhwp WASM once per page. The module is a same-origin build
 * asset; its bytes are fetched and handed to `init` (source `rhwp-init.ts`),
 * so compilation needs only `'wasm-unsafe-eval'`. A failed attempt is not
 * cached, so a retry fetches again.
 */
export function ensureRhwpCore(): Promise<void> {
  ready ??= (async () => {
    installMeasureTextWidth();
    const response = await fetch(wasmUrl, { credentials: "same-origin" });
    if (!response.ok) {
      await response.body?.cancel();
      throw new Error(`rhwp wasm ${response.status}`);
    }
    await init({ module_or_path: await response.arrayBuffer() });
  })();
  ready.catch(() => {
    ready = null;
  });
  return ready;
}
