import assert from "node:assert/strict";
import test from "node:test";
import { embedSlashItems, filterSlashItems } from "../src/react/suggestion-menu.ts";

// The template presentation must leave the authorized async producers and
// custom FVOCI commands discoverable, including when only an alias matches.
test("grouped slash presentation preserves all existing command aliases and insertion callbacks", () => {
  const commands = filterSlashItems("");
  for (const alias of ["h1", "h2", "h3", "p", "ul", "ol", "quote", "code", "table", "hr", "task", "callout", "toggle", "math", "mermaid", "attachment"]) {
    const command = commands.find((item) => item.aliases.includes(alias));
    assert.ok(command, alias);
    assert.equal(filterSlashItems(alias).some((item) => item.title === command.title), true, alias);
    assert.equal(typeof command.run, "function", alias);
    assert.ok(command.group, alias);
  }
  assert.notEqual(filterSlashItems("math")[0]?.group, filterSlashItems("h1")[0]?.group);
  assert.equal(filterSlashItems("unknown-no-match").length, 0);
});

test("template presentation leaves entity and URL embeds in the existing producer", () => {
  const hits = [
    { entity: "document", id: "d", label: "문서" },
    { entity: "task", id: "t", label: "태스크" },
    { entity: "user", id: "u", label: "사용자" },
  ];
  const items = embedSlashItems("https://example.com/한글", hits);
  assert.equal(items.length, 3, "only the URL and supported entity kinds");
  assert.ok(items[0]?.aliases.includes("url"));
  assert.ok(items[1]?.title.includes("문서"));
  assert.ok(items[2]?.title.includes("태스크"));
});
