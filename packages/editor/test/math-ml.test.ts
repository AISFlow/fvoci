import assert from "node:assert/strict";
import test from "node:test";
import { renderMathMl, withoutStyleAttributes } from "../src/math-ml.ts";

// No DOM here (bun test): the strip must not need one, since an in-page parse
// of the styled markup is what reported the CSP violation.
test("KaTeX output that carries style= comes back without it", async () => {
  for (const latex of ["\\frac{", "{", "\\pmb{x}", "\\fcolorbox{red}{blue}{x}"]) {
    const { html, failed } = await renderMathMl(latex, true);
    assert.equal(failed, false, latex);
    assert.ok(html, latex);
    assert.equal(/\sstyle=/.test(html), false, `${latex}: ${html}`);
  }
  const error = await renderMathMl("\\frac{", true);
  assert.match(error.html ?? "", /class="katex-error"/);
});

test("quoted style text in the source stays text", async () => {
  const { html } = await renderMathMl('\\text{ style="x"}', false);
  // KaTeX turns the leading space into U+00A0.
  assert.match(html ?? "", /<mtext>\sstyle=&quot;x&quot;<\/mtext>/);
});

test("the strip removes only attributes", () => {
  assert.equal(
    withoutStyleAttributes('<span class="a" style="color:#cc0000">style="b"</span>'),
    '<span class="a">style="b"</span>',
  );
});
