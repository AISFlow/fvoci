import assert from "node:assert/strict";
import test from "node:test";
import {
  isHttpUrl,
  isSandboxedIframeHtml,
  unfurlCardDataOf,
  unfurlDisplayTitle,
  type UnfurlOutput,
} from "./unfurl.ts";

function og(partial: Partial<UnfurlOutput>): UnfurlOutput {
  return {
    kind: "og",
    url: "https://example.com",
    title: null,
    description: null,
    imageUrl: null,
    state: null,
    number: null,
    owner: null,
    repo: null,
    ...partial,
  };
}

await test("http(s)만 허용", () => {
  assert.equal(isHttpUrl("https://github.com/fvoci/FVOCI"), true);
  assert.equal(isHttpUrl("javascript:alert(1)"), false);
  assert.equal(isHttpUrl("not-a-url"), false);
});

await test("OG 필드·비http 이미지는 버린다", () => {
  assert.deepEqual(
    unfurlCardDataOf(
      og({
        title: " FVOCI ",
        description: "설명",
        imageUrl: "javascript:alert(1)",
      }),
    ),
    { title: "FVOCI", description: "설명", imageUrl: null },
  );
  assert.deepEqual(unfurlCardDataOf(og({ title: "G", imageUrl: "https://example.com/og.png" })), {
    title: "G",
    description: "",
    imageUrl: "https://example.com/og.png",
  });
});

await test("GitHub 제목 폴백 owner/repo#n", () => {
  assert.equal(
    unfurlCardDataOf(
      og({
        kind: "github_issue",
        owner: "fvoci",
        repo: "FVOCI",
        number: 12,
      }),
    ).title,
    "fvoci/FVOCI#12",
  );
});

await test("제목 없으면 호스트", () => {
  assert.equal(unfurlDisplayTitle("", "https://github.com/fvoci/FVOCI"), "github.com");
});

await test("sandbox iframe HTML만 임베드로 쓴다", () => {
  assert.equal(
    isSandboxedIframeHtml(
      '<iframe src="https://www.youtube.com/embed/x" sandbox="allow-scripts allow-same-origin"></iframe>',
    ),
    true,
  );
  assert.equal(isSandboxedIframeHtml('<div onclick="alert(1)">x</div>'), false);
  assert.equal(isSandboxedIframeHtml('<iframe src="https://evil.example"></iframe>'), false);
});
