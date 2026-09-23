import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { PRESENCE_COLORS, presenceColorOf } from "../../lib/presence.ts";
import { collabUserOf } from "./collab-model.ts";

const sessionPath = path.join(import.meta.dirname, "collab-session.tsx");
const cssPath = path.join(import.meta.dirname, "../../styles/app.css");

test("collab-session 은 provider-react 를 쓴다", () => {
  const src = readFileSync(sessionPath, "utf8");
  assert.equal(src.includes("@hocuspocus/provider-react"), true);
  assert.equal(src.includes("new HocuspocusProvider"), false);
});

test("collab-session 에 hex 리터럴이 없다", () => {
  const src = readFileSync(sessionPath, "utf8").replace(
    /\/\*[\s\S]*?\*\/|\/\/.*/g,
    "",
  );
  assert.equal(/#[0-9a-fA-F]{3,8}/.test(src), false);
});

test("collabUserOf 색은 .afn-label-* --afn-label-ink 이다", () => {
  const user = collabUserOf("01a01f00-0000-7000-8000-000000000001", "김연구");
  assert.match(user.color, /^#[0-9a-fA-F]{6}$/);
  const css = readFileSync(cssPath, "utf8");
  assert.equal(css.includes(`--afn-label-ink: ${user.color}`), true);
});

const OLD_LABEL_KEYS = [
  "red",
  "orange",
  "amber",
  "green",
  "teal",
  "blue",
  "violet",
  "pink",
] as const;

function oldClientIndex(userId: string): number {
  const hex = userId.replaceAll("-", "").slice(-6);
  const parsed = Number.parseInt(hex, 16);
  return Number.isFinite(parsed) ? parsed % OLD_LABEL_KEYS.length : 0;
}

function labelInk(css: string, key: string): string | undefined {
  return new RegExp(
    `^\\.afn-label-${key} \\{[^}]*--afn-label-ink: (#[0-9a-fA-F]{6})`,
    "m",
  ).exec(css)?.[1];
}

test("presenceColorOf 는 옛 클라이언트 인덱스와 같은 .afn-label-* 잉크를 고른다", () => {
  const css = readFileSync(cssPath, "utf8");
  assert.equal(PRESENCE_COLORS.length, OLD_LABEL_KEYS.length);
  for (let i = 0; i < OLD_LABEL_KEYS.length; i += 1) {
    const userId = `01a01f00-0000-7000-8000-00000000000${i.toString(16)}`;
    const key = OLD_LABEL_KEYS[oldClientIndex(userId)] ?? "";
    assert.equal(oldClientIndex(userId), i);
    assert.equal(labelInk(css, key), PRESENCE_COLORS[i]);
    assert.equal(presenceColorOf(userId), labelInk(css, key));
  }
});

test("다른 uuid 뒷자리는 다른 라벨 색을 고른다", () => {
  const a = collabUserOf("01a01f00-0000-7000-8000-000000000001", "김");
  const b = collabUserOf("01a01f00-0000-7000-8000-00000000000b", "박");
  assert.notEqual(a.color, b.color);
  assert.match(a.color, /^#[0-9a-fA-F]{6}$/);
  assert.match(b.color, /^#[0-9a-fA-F]{6}$/);
});
