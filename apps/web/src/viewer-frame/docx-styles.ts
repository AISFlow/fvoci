/**
 * How much style sheet text docx-preview would write for a Word document's
 * styles, at most, reckoned from its model before it draws.
 *
 * docx-preview writes the style sheets as text, not as nodes, so the
 * worker's count of nodes does not see them. It writes a rule for each of a
 * style's entries; it first copies into each style what the style it is
 * based on has (`processStyles`), and adds to each the entries of the style
 * it is linked to (`renderStyles`); each rule's selector carries the
 * style's id. A few styles linked to one long one can so ask for gigabytes.
 * The reckoning follows that and errs high: a style counts all the entries
 * it has and its chain has, and all its linked style counts, each with a
 * selector of its own id (at three times its length: the class name made of
 * it can grow so).
 *
 * The style sheet docx-preview writes for numberings is reckoned with the
 * numbering part, as it is read (`worker-xml.ts`).
 */

/** The most style sheet text a document may ask for, in characters: hundreds of times a long report's. */
export const STYLE_TEXT_LIMIT = 8_000_000;

type Model = Record<string, unknown>;
const isModel = (value: unknown): value is Model => typeof value === 'object' && value !== null;
const listOf = (value: unknown): unknown[] => (Array.isArray(value) ? value : []);
/** What a rule adds to its selector's id: the class prefix, the targets, the braces. */
const RULE = 60;

/** Entries, and their declarations' text. */
interface Held {
  entries: number;
  text: number;
}

/** The text of one entry's declarations. */
function entryText(entry: unknown): number {
  const values = isModel(entry) ? entry.values : undefined;
  let length = 0;
  if (isModel(values)) {
    for (const [name, value] of Object.entries(values))
      length += name.length + String(value).length + 4;
  }
  return length;
}

/** The style sheet text the styles would ask for, at most; 0 for a model without styles. */
export function styleText(document: unknown): number {
  const stylesPart = isModel(document) ? document.stylesPart : undefined;
  const styles = listOf(isModel(stylesPart) ? stylesPart.styles : undefined).filter(isModel);
  const byId = new Map<string, Model>();
  const own = new Map<Model, Held>();
  for (const style of styles) {
    if (typeof style.id === 'string') byId.set(style.id, style);
    const entries = listOf(style.styles);
    own.set(style, {
      entries: entries.length,
      text: entries.reduce<number>((sum, entry) => sum + entryText(entry), 0),
    });
  }
  const based = (style: Model): Model | undefined =>
    typeof style.basedOn === 'string' ? byId.get(style.basedOn) : undefined;

  // What each style can hold once its chain is copied in: its own and all its chain's.
  const held = new Map<Model, Held>();
  for (const style of styles) {
    const chain: Model[] = [];
    let at: Model | undefined = style;
    // A chain no longer than the styles there are: a loop of "based on" ends.
    while (at && !held.has(at) && chain.length <= styles.length) {
      chain.push(at);
      at = based(at);
    }
    const below = { ...((at && held.get(at)) ?? { entries: 0, text: 0 }) };
    for (const each of chain.toReversed()) {
      const mine = own.get(each) ?? { entries: 0, text: 0 };
      below.entries += mine.entries;
      below.text += mine.text;
      held.set(each, { ...below });
    }
  }
  let total = 0;
  for (const style of styles) {
    const selector = RULE + 3 * (typeof style.id === 'string' ? style.id.length : 0);
    const linked = typeof style.linked === 'string' ? byId.get(style.linked) : undefined;
    for (const part of [held.get(style), linked && held.get(linked)]) {
      if (part) total += part.text + part.entries * selector;
    }
  }
  return total;
}
