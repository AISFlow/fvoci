/**
 * In place of the editor's CanvasKit renderer (rhwp-studio,
 * view/canvaskit-renderer.ts; the alias in vite.config.ts).
 *
 * The editor draws on the browser's own canvas unless its address asks for
 * CanvasKit, and the frame's never does. Left in, the renderer would bring
 * Skia's WebAssembly (7 MB) into the build for a path nobody takes; this
 * takes its place, and says so should the path ever be taken.
 */
export const CanvasKitLayerRenderer = {
  create(): Promise<never> {
    return Promise.reject(new Error('the CanvasKit renderer is not built into this editor'));
  },
};
