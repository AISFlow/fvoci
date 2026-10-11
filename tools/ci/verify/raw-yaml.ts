// Raw-source lookup of one block-style scalar, for checks whose meaning the
// YAML 1.2 value model loses (20.0 vs 20). Bun.YAML resolves duplicate keys
// silently and PyYAML keeps the last one, so the lookup is fail closed: every
// key line along the path is decoded with YAML quoting rules, and any
// duplicate spelling of a path key, any line it cannot classify, or any
// non-plain form of the target returns null.

const DOUBLE_QUOTED_ESCAPES: Readonly<Record<string, string>> = {
  "0": "\0",
  a: "\x07",
  b: "\b",
  t: "\t",
  "\t": "\t",
  n: "\n",
  v: "\v",
  f: "\f",
  r: "\r",
  e: "\x1b",
  " ": " ",
  '"': '"',
  "/": "/",
  "\\": "\\",
  N: "\x85",
  _: "\xa0",
  L: " ",
  P: " ",
};
const HEX_ESCAPES: Readonly<Record<string, number>> = { x: 2, u: 4, U: 8 };

type KeyLine = { key: string; plain: boolean; rest: string };

function decodeDoubleQuoted(text: string): { key: string; end: number } | null {
  let key = "";
  for (let i = 1; i < text.length; i += 1) {
    const char = text[i] as string;
    if (char === '"') return { key, end: i + 1 };
    if (char !== "\\") {
      key += char;
      continue;
    }
    const escape = text[i + 1];
    if (escape === undefined) return null;
    const width = HEX_ESCAPES[escape];
    if (width !== undefined) {
      const hex = text.slice(i + 2, i + 2 + width);
      if (!new RegExp(`^[0-9A-Fa-f]{${String(width)}}$`).test(hex)) return null;
      const code = Number.parseInt(hex, 16);
      if (code > 0x10ffff) return null;
      key += String.fromCodePoint(code);
      i += 1 + width;
    } else {
      const decoded = DOUBLE_QUOTED_ESCAPES[escape];
      if (decoded === undefined) return null;
      key += decoded;
      i += 1;
    }
  }
  return null;
}

function decodeSingleQuoted(text: string): { key: string; end: number } | null {
  let key = "";
  for (let i = 1; i < text.length; i += 1) {
    const char = text[i] as string;
    if (char !== "'") {
      key += char;
      continue;
    }
    if (text[i + 1] === "'") {
      key += "'";
      i += 1;
      continue;
    }
    return { key, end: i + 1 };
  }
  return null;
}

// Plain keys may not start with an indicator; anything else (complex `?`
// keys, anchors, aliases, tags, flow collections, merge keys) is unclassified.
const PLAIN_KEY = /^[^\s?&*!|>'"%@`{}[\],#:-][^\n]*?$|^-[^\s\n][^\n]*?$/;

function parseKeyLine(text: string): KeyLine | null {
  let key: string;
  let after: string;
  let plain = false;
  if (text.startsWith('"') || text.startsWith("'")) {
    const quoted = text.startsWith('"') ? decodeDoubleQuoted(text) : decodeSingleQuoted(text);
    if (quoted === null) return null;
    key = quoted.key;
    after = text.slice(quoted.end).replace(/^[ \t]+/, "");
    if (!after.startsWith(":")) return null;
    after = after.slice(1);
  } else {
    const colon = text.search(/:([ \t]|$)/);
    if (colon <= 0) return null;
    key = text.slice(0, colon).replace(/[ \t]+$/, "");
    if (!PLAIN_KEY.test(key) || key === "<<" || /[ \t]#/.test(key)) return null;
    after = text.slice(colon + 1);
    plain = true;
  }
  if (after !== "" && !/^[ \t]/.test(after)) return null;
  const comment = after.search(/(^|[ \t])#/);
  const rest = (comment === -1 ? after : after.slice(0, comment)).replace(/^[ \t]+|[ \t]+$/g, "");
  return { key, plain, rest };
}

const indentOf = (line: string) => line.length - line.replace(/^ +/, "").length;
const isContent = (line: string) => line.trim() !== "" && !line.trimStart().startsWith("#");

export function rawBlockScalar(source: string, path: readonly string[]): string | null {
  let block = source.split("\n").map((line) => line.replace(/\r$/, ""));
  let parentIndent = -1;
  for (const [depth, segment] of path.entries()) {
    const content = block.filter(isContent);
    if (content.some((line) => /^ *\t/.test(line))) return null;
    const indent = content.length > 0 ? indentOf(content[0] as string) : -1;
    if (indent <= parentIndent) return null;
    let found = -1;
    let foundLine: KeyLine | null = null;
    for (const [index, line] of block.entries()) {
      if (!isContent(line)) continue;
      const lineIndent = indentOf(line);
      if (lineIndent < indent) return null;
      if (lineIndent > indent) continue;
      const parsed = parseKeyLine(line.slice(indent));
      if (parsed === null) return null;
      if (parsed.key !== segment) continue;
      if (foundLine !== null) return null;
      found = index;
      foundLine = parsed;
    }
    if (foundLine === null) return null;
    if (depth === path.length - 1)
      return foundLine.plain && foundLine.rest !== "" ? foundLine.rest : null;
    if (foundLine.rest !== "") return null;
    const child: string[] = [];
    for (const line of block.slice(found + 1)) {
      if (isContent(line) && indentOf(line) <= indent) break;
      child.push(line);
    }
    block = child;
    parentIndent = indent;
  }
  return null;
}
