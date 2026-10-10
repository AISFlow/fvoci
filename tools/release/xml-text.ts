// Character data (text and CDATA) of a namespace-well-formed XML document.
//
// saxes (pinned by apps/web, installed by `bun ci`) does the parsing: any
// well-formedness error throws, including bad attribute syntax, duplicate
// attributes, undefined entities, character references outside the XML Char
// production, unbound namespace prefixes, mismatched or unclosed tags and
// content outside the root. Only the five predefined entities resolve.
import { SaxesParser } from "saxes";

export function xmlText(xml: string): string {
  const parser = new SaxesParser({ xmlns: true, position: true });
  // OOXML parts are XML 1.0; a 1.1 declaration would admit control
  // characters such as &#1;.
  parser.on("xmldecl", (decl) => {
    if (decl.version !== "1.0") throw parser.makeError(`XML version ${String(decl.version)}`);
  });
  // OPC (ISO/IEC 29500-2) forbids DTD declarations in package XML, and saxes
  // does not check DTD syntax, so every DOCTYPE is refused, not skipped.
  parser.on("doctype", () => {
    throw parser.makeError("DOCTYPE is not allowed in an Office part");
  });
  let text = "";
  let depth = 0;
  parser.on("opentag", (tag) => {
    if (!tag.isSelfClosing) depth++;
  });
  parser.on("closetag", (tag) => {
    if (!tag.isSelfClosing) depth--;
  });
  // Whitespace around the root element is not document text.
  parser.on("text", (chunk) => {
    if (depth > 0) text += chunk;
  });
  parser.on("cdata", (chunk) => {
    text += chunk;
  });
  parser.write(xml).close();
  return text;
}
