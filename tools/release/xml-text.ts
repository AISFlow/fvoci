// Character data of an XML document (text and CDATA, without comments,
// processing instructions or markup), refusing unbalanced or unclosed
// elements, unbound namespace prefixes and unknown entities. Enough to find a
// phrase in OOXML parts; not a validating parser (no DTD entities, attribute
// syntax unchecked).
const PREDEFINED: Record<string, string> = {
  amp: "&",
  lt: "<",
  gt: ">",
  quot: '"',
  apos: "'",
};

function decodeEntities(text: string): string {
  return text.replace(/&([^;&\s]*);|&/g, (match, ref: string | undefined) => {
    if (ref === undefined) throw new Error("bare & in XML text");
    const numeric = /^#(?:x([0-9a-fA-F]+)|([0-9]+))$/.exec(ref);
    if (numeric)
      return String.fromCodePoint(parseInt(numeric[1] ?? numeric[2] ?? "", numeric[1] ? 16 : 10));
    const named = PREDEFINED[ref];
    if (named === undefined) throw new Error(`undefined XML entity ${match}`);
    return named;
  });
}

// Namespace prefixes an element declares, after checking that its own and its
// attributes' prefixes are bound in scope.
function bindPrefixes(name: string, attributes: string, scope: Set<string>): Set<string> {
  const declared = new Set(scope);
  const attrs = [...attributes.matchAll(/([^\s=]+)\s*=\s*(?:"[^"]*"|'[^']*')/g)].map(
    (m) => m[1] ?? "",
  );
  for (const attr of attrs) {
    if (attr.startsWith("xmlns:")) declared.add(attr.slice("xmlns:".length));
  }
  for (const qname of [name, ...attrs.filter((a) => a !== "xmlns" && !a.startsWith("xmlns:"))]) {
    const colon = qname.indexOf(":");
    if (colon > 0 && !declared.has(qname.slice(0, colon))) {
      throw new Error(`unbound prefix in ${qname}`);
    }
  }
  return declared;
}

export function xmlText(xml: string): string {
  const open: string[] = [];
  const scopes: Array<Set<string>> = [new Set(["xml"])];
  let text = "";
  let rootClosed = false;
  const token =
    /<!--[\s\S]*?-->|<\?[\s\S]*?\?>|<!\[CDATA\[([\s\S]*?)\]\]>|<!DOCTYPE[^>]*>|<(\/?)([^\s/>]+)((?:[^>"']|"[^"]*"|'[^']*')*?)(\/?)>|([^<]+)|</g;
  for (const m of xml.matchAll(token)) {
    const [whole, cdata, closing, name, attributes, selfClosing, chars] = m;
    if (chars !== undefined) {
      if (open.length) text += decodeEntities(chars);
      else if (chars.trim()) throw new Error("text outside the root element");
    } else if (cdata !== undefined) {
      if (!open.length) throw new Error("CDATA outside the root element");
      text += cdata;
    } else if (name !== undefined) {
      if (closing) {
        if (open.pop() !== name) throw new Error(`mismatched </${name}>`);
        scopes.pop();
        if (!open.length) rootClosed = true;
      } else {
        if (rootClosed || (!open.length && text)) throw new Error("more than one root element");
        const scope = bindPrefixes(name, attributes ?? "", scopes.at(-1) ?? new Set());
        if (!selfClosing) {
          open.push(name);
          scopes.push(scope);
        } else if (!open.length) rootClosed = true;
      }
    } else if (whole === "<") {
      throw new Error("unterminated markup");
    }
  }
  if (open.length || !rootClosed) throw new Error("unclosed XML element");
  return text;
}
