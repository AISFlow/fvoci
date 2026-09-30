import { type Ref, shallowRef, watch } from "vue";
import { type MathRender, mathMlWithoutKatex, renderMathMl } from "../math-ml.js";

/** MathML for `latex`, rendered by the lazily loaded KaTeX chunk (math-ml.ts).
 * Empty and over-limit sources settle without a tick; a render that finishes
 * after the source changed again is dropped. */
export function useMathMl(
  latex: Readonly<Ref<string>>,
  display: boolean,
): Readonly<Ref<MathRender>> {
  const render = shallowRef<MathRender>({ html: null, failed: false });
  watch(
    latex,
    async (value, _previous, onCleanup) => {
      const settled = mathMlWithoutKatex(value);
      if (settled) {
        render.value = settled;
        return;
      }
      let alive = true;
      onCleanup(() => {
        alive = false;
      });
      const next = await renderMathMl(value, display);
      if (alive) render.value = next;
    },
    { immediate: true },
  );
  return render;
}
