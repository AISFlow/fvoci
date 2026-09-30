import assert from "node:assert/strict";
import test from "node:test";
import { guardedViewerLink } from "./viewer-navigation";

const current = "https://fvoci.example/w/acme/a/123/view?chunk=0";
const click = {
  button: 0,
  metaKey: false,
  ctrlKey: false,
  shiftKey: false,
  altKey: false,
  defaultPrevented: false,
};
const link = { href: "/w/acme/search?q=hello#results", target: "", download: false };

await test("a local viewer link enters route guards with its complete destination", () => {
  assert.equal(guardedViewerLink(click, link, current), link.href);
  assert.equal(
    guardedViewerLink(click, { ...link, href: "/w/acme/a/456/view", target: "_self" }, current),
    "/w/acme/a/456/view",
  );
});

await test("viewer guards preserve modified clicks, new tabs, downloads, external links and document anchors", () => {
  for (const field of ["metaKey", "ctrlKey", "shiftKey", "altKey", "defaultPrevented"]) {
    assert.equal(guardedViewerLink({ ...click, [field]: true }, link, current), null, field);
  }
  assert.equal(guardedViewerLink({ ...click, button: 1 }, link, current), null);
  for (const target of ["_blank", "_parent", "_top", "other-window"]) {
    assert.equal(guardedViewerLink(click, { ...link, target }, current), null, target);
  }
  assert.equal(guardedViewerLink(click, { ...link, download: true }, current), null);
  for (const href of [
    "https://other.example/path",
    "mailto:user@example.com",
    "javascript:void(0)",
    "#page-2",
  ]) {
    assert.equal(guardedViewerLink(click, { ...link, href }, current), null, href);
  }
});
