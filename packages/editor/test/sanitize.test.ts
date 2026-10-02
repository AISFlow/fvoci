import assert from "node:assert/strict";
import test from "node:test";
import { tiptapDocToSafeHtml } from "../src/html.ts";
import { sanitizeRenderedHtml } from "../src/sanitize.ts";

await test("existing stored-document HTML producer retains headings, filenames, references and escaped literal hostile data at the shared sanitizer", () => {
  const html = tiptapDocToSafeHtml({
    type: "doc",
    content: [
      {
        type: "heading",
        attrs: { level: 4 },
        content: [{ type: "text", text: "한글 level four" }],
      },
      { type: "heading", attrs: { level: 5 }, content: [{ type: "text", text: "five" }] },
      { type: "heading", attrs: { level: 6 }, content: [{ type: "text", text: "six" }] },
      { type: "attachment", attrs: { id: "file-id", name: "자료 <script>literal</script>.pdf" } },
      { type: "embed", attrs: { entity: "task", ref: 'task-target<&"' } },
      {
        type: "paragraph",
        content: [
          {
            type: "text",
            text: "링크",
            marks: [{ type: "link", attrs: { href: "javascript:evil()" } }],
          },
        ],
      },
    ],
  });
  assert.ok(html.includes("<h4>한글 level four</h4>"));
  assert.ok(html.includes("<h5>five</h5>") && html.includes("<h6>six</h6>"));
  assert.ok(html.includes("자료 &lt;script&gt;literal&lt;/script&gt;.pdf"));
  assert.ok(html.includes('task-target&lt;&amp;"'));
  assert.equal(html.includes("<script>"), false);
  assert.equal(html.includes("javascript:"), false);
});

await test("heading/callout/table producer markup preserves semantic levels, kind, geometry and only bounded presentation", () => {
  const html = sanitizeRenderedHtml(
    '<h4 data-id="h" style="text-align:justify">제목</h4><aside class="afn-callout" data-callout="" data-kind="warning"><p>주의</p></aside><table style="width:300px"><colgroup><col style="width:120px"><col style="min-width:180px"></colgroup><tbody><tr><th colspan="2" rowspan="2" colwidth="120,180" data-colwidth="120,180" data-background="#abcdef" style="text-align:center;background:#abcdef"><p>셀</p></th></tr></tbody></table>',
  );
  assert.ok(html.includes('<h4 data-id="h" style="text-align:justify">'));
  assert.ok(html.includes("data-callout") && html.includes('data-kind="warning"'));
  assert.ok(
    html.includes(
      '<colgroup><col style="width:120px" /><col style="min-width:180px" /></colgroup>',
    ),
  );
  assert.ok(
    html.includes('colspan="2"') &&
      html.includes('rowspan="2"') &&
      html.includes('colwidth="120,180"'),
  );
  assert.ok(html.includes("text-align:center;background:#abcdef"));
});

await test("actual FVOCI color tokens and bounded literal colors remain useful without allowing arbitrary CSS functions", () => {
  for (const color of [
    "var(--accent)",
    "var(--destructive)",
    "var(--muted)",
    "#112233",
    "#abc",
    "rgb(17, 34, 51)",
    "rgba(17, 34, 51, 0.5)",
  ]) {
    const html = sanitizeRenderedHtml(
      `<span style="color:${color}">색</span><mark data-color="${color}" style="background-color:${color};color:inherit">강조</mark><td style="background:${color}">셀</td>`,
    );
    assert.ok(html.includes(`color:${color}`), color);
    assert.ok(html.includes(`background-color:${color}`), color);
    assert.ok(html.includes(`background:${color}`), color);
    assert.ok(html.includes("color:inherit"), color);
  }
  for (const color of [
    "var(--unknown)",
    "var(--accent,url(https://evil.test/))",
    "url(https://evil.test/)",
    "expression(evil())",
    "rgb(999,0,0)",
    "rgba(0,0,0,2)",
  ]) {
    const html = sanitizeRenderedHtml(
      `<span style="color:${color};background-color:${color}">색</span><td style="background:${color}">셀</td>`,
    );
    assert.equal(html.includes("style="), false, color);
  }
});

await test("newly styled headings/cells/columns do not inherit existing math positioning or unrelated CSS properties", () => {
  for (const tag of ["p", "h1", "h4", "h6", "mark", "td", "th", "table", "col"]) {
    const html = sanitizeRenderedHtml(
      `<${tag} style="position:absolute;top:0px;left:0px;height:999px;margin-left:999px;background-image:url(https://evil.test/);font-family:evil" onclick="evil()">text</${tag}>`,
    );
    assert.equal(html.includes("style="), false, tag);
    assert.equal(html.includes("onclick"), false, tag);
  }
  // Existing KaTeX numeric presentation stays available on its existing span.
  assert.ok(
    sanitizeRenderedHtml('<span style="vertical-align:-0.5em;height:1em">수식</span>').includes(
      "vertical-align:-0.5em;height:1em",
    ),
  );
});

await test("URI, image and non-text policies still reject active data while keeping actual allowed attachment URLs", () => {
  const download =
    "/api/v1/workspaces/10000000-0000-4000-8000-000000000001/attachments/10000000-0000-4000-8000-000000000009/download";
  const html = sanitizeRenderedHtml(
    `<a href="${download}" onclick="evil()">자료.pdf</a><a href="jav&#x61;script:evil()">bad</a><a href="//evil.test">relative</a><img src="data:text/html,evil" onerror="evil()"><img src="${download}?variant=preview" alt="한글"><script>active()</script><style>body{display:none}</style><iframe src="https://evil.test"></iframe><textarea>private</textarea>`,
  );
  assert.ok(html.includes(`href="${download}"`));
  assert.ok(html.includes(`src="${download}?variant=preview"`) && html.includes('alt="한글"'));
  assert.equal(html.includes("javascript:"), false);
  assert.equal(html.includes("//evil.test"), false);
  assert.equal(html.includes("data:text/html"), false);
  assert.equal(html.includes("onerror"), false);
  assert.equal(html.includes("onclick"), false);
  assert.equal(html.includes("active()"), false);
  assert.equal(html.includes("display:none"), false);
  assert.equal(html.includes("<iframe"), false);
  assert.equal(html.includes("private"), false);
});

await test("duplicate/mixed-case attributes and malformed escaped markup cannot smuggle URI/events/CSS into preview", () => {
  for (const attributes of [
    'href="javascript:evil()" HREF="https://example.com"',
    'HREF="https://example.com" href="javascript:evil()"',
    'href="&#106;&#97;vascript:evil()" onclick="evil()"',
    'href="javascript:evil()" href="javascript:other()"',
  ]) {
    const html = sanitizeRenderedHtml(`<a ${attributes}>문자 🧑‍💻</a>`);
    assert.ok(html.includes("문자 🧑‍💻"));
    assert.equal(html.includes("javascript:"), false);
    assert.equal(html.includes("onclick"), false);
    assert.equal(html.includes("evil()"), false);
  }
  const escaped = sanitizeRenderedHtml(
    '<p>&lt;img src=x onerror=evil()&gt; &amp; 한글</p><span style="color:#112233" STYLE="color:expression(evil())" onmouseover="evil()">safe</span>',
  );
  assert.ok(escaped.includes("&lt;img src=x onerror=evil()&gt; &amp; 한글"));
  assert.equal(escaped.includes("<img"), false);
  assert.equal(escaped.includes("expression"), false);
  assert.equal(escaped.includes("onmouseover"), false);
});
