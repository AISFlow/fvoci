/**
 * Extra correctness vectors made with the locked web dependencies. Only uses the
 * binding and ProseMirror's data model, without mounting an editor or writing to a
 * dependency directory. The schema's attributes/order come from the web contract;
 * its content expressions below mirror the product's node extensions.
 *
 * node crates/collab/tests/review_binding.mjs --modules apps/web/node_modules --write
 * Omit --write to check that the committed fixture can be reproduced.
 * To check Rust-authored states too, run from the workspace root:
 * FVOCI_REVIEW_BINDING_OUTPUT="$PWD/target/review-binding-rust.json" \
 *   CARGO_BUILD_JOBS=6 cargo test -p fvoci-collab --test review_round_trip extra_binding_vectors
 * Then pass --rust-state target/review-binding-rust.json to this script.
 */
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '../../..');
const at = process.argv.indexOf('--modules');
const modules = at < 0 ? path.join(root, 'apps/web/node_modules') : path.resolve(process.argv[at + 1]);
const require = createRequire(path.join(modules, '__review__.cjs'));
const Y = require('yjs');
const { Schema, Node } = require('@tiptap/pm/model');
const { prosemirrorToYXmlFragment, yXmlFragmentToProseMirrorRootNode } = require('@tiptap/y-tiptap');
const contract = JSON.parse(readFileSync(path.join(root, 'contracts/ydoc-vectors.json'), 'utf8'));
const locked = JSON.parse(readFileSync(path.join(root, 'apps/web/package.json'), 'utf8')).dependencies;
const versions = Object.fromEntries(['yjs', '@tiptap/y-tiptap', '@tiptap/pm'].map(name => {
  const version = JSON.parse(readFileSync(path.join(modules, name, 'package.json'), 'utf8')).version;
  assert.equal(version, locked[name], `${name}: use the locked web version`);
  return [name, version];
}));
const content = {
  doc: 'block+', paragraph: 'inline*', heading: 'inline*', blockquote: 'block+',
  bulletList: 'listItem+', orderedList: 'listItem+', taskList: 'taskItem+',
  listItem: 'paragraph block*', taskItem: 'paragraph block*', codeBlock: 'text*',
};
const inline = new Set(['text', 'hardBreak', 'mention', 'reference', 'inlineMath']);
const attrs = given => Object.fromEntries(Object.entries(given).map(([key, value]) => [key, { default: value }]));
const schema = new Schema({
  nodes: Object.fromEntries(Object.entries(contract.schema.nodes).map(([name, given]) => [name, {
    ...(name === 'doc' ? {} : { group: inline.has(name) ? 'inline' : 'block' }),
    ...(inline.has(name) && name !== 'text' ? { inline: true } : {}),
    ...(content[name] ? { content: content[name] } : {}),
    ...(name === 'codeBlock' ? { marks: '' } : {}),
    attrs: attrs(given),
  }])),
  marks: Object.fromEntries(contract.schema.marks.map(mark => [mark.name, { attrs: attrs(mark.attrs) }])),
});
const doc = blocks => ({ type: 'doc', content: blocks });
const text = (value, marks = []) => ({ type: 'text', text: value, ...(marks.length ? { marks } : {}) });
const paragraph = (...runs) => ({ type: 'paragraph', ...(runs.length ? { content: runs } : {}) });
const create = (body, id) => {
  const result = new Y.Doc(); result.clientID = id;
  prosemirrorToYXmlFragment(Node.fromJSON(schema, body), result.getXmlFragment('default'));
  return result;
};
const fork = (base, id) => {
  const result = new Y.Doc(); result.clientID = id;
  Y.applyUpdate(result, Y.encodeStateAsUpdate(base)); return result;
};
const stored = node => {
  const out = { type: node.type };
  const given = Object.entries(node.attrs ?? {}).filter(([, value]) => value != null);
  if (given.length) out.attrs = Object.fromEntries(given);
  if (node.content?.length) out.content = node.content.map(stored);
  if (node.marks?.length) out.marks = node.marks.map(stored);
  if (node.text !== undefined) out.text = node.text;
  return out;
};
const read = source => {
  const copy = fork(source, 9);
  const before = Buffer.from(Y.encodeStateAsUpdate(copy));
  const result = stored(yXmlFragmentToProseMirrorRootNode(copy.getXmlFragment('default'), schema).toJSON());
  assert.deepEqual(Buffer.from(Y.encodeStateAsUpdate(copy)), before, 'the binding must keep these valid bodies');
  return result.content?.length === 1 && result.content[0].type === 'paragraph' && !result.content[0].content ? null : result;
};
const letters = '😀e\u0301한글𝒳👩‍👩‍👧‍👦\r\n\t\u2028\u2029';
const cases = [];
for (let mask = 0; mask < 32; mask++) {
  const marks = contract.schema.marks.filter((_, index) => mask & (1 << index)).map(mark => (
    mark.name === 'link' ? { type: 'link', attrs: { href: 'https://example.com/😀e\u0301' } } : { type: mark.name }
  ));
  cases.push({ name: `Unicode mark set ${mask}`, body: doc([
    paragraph(text('plain'), text(letters, marks), { type: 'hardBreak' }, text('end', marks)),
  ]) });
}
cases.push({ name: 'Unicode in attributes, empty optional strings, defaults and Mermaid', body: doc([
  { type: 'heading', attrs: { level: 3 }, content: [text(letters)] },
  paragraph({ type: 'inlineMath', attrs: { latex: letters } },
    { type: 'mention', attrs: { userId: '01930000-0000-7000-8000-00000000c002' } },
    { type: 'reference', attrs: { id: '01930000-0000-7000-8000-00000000b002' } }),
  { type: 'blockMath', attrs: { latex: letters } },
  { type: 'image', attrs: { fileId: 'unicode-image.png', alt: letters } },
  { type: 'image', attrs: { fileId: 'empty-alt.png', alt: '' } },
  { type: 'file', attrs: { fileId: 'unicode-doc.pdf' } },
  { type: 'codeBlock', attrs: { language: 'mermaid' }, content: [text('graph TD\n A["😀e\u0301"]-->B["한글"]')] },
  { type: 'codeBlock' },
  { type: 'orderedList', content: [{ type: 'listItem', content: [paragraph()] }] },
]) });
const result = cases.map(({ name, body }) => {
  const made = create(body, 1);
  return { name, body, json: read(made), update: Buffer.from(Y.encodeStateAsUpdate(made)).toString('base64') };
});
const output = JSON.stringify({
  description: 'Additional normal-body vectors from review_binding.mjs using the locked yjs and y-tiptap versions; no editor rendering or repair oracle.',
  versions,
  cases: result,
}, null, 2) + '\n';
const fixture = path.join(here, 'data/review-binding.json');
if (process.argv.includes('--write')) writeFileSync(fixture, output);
else assert.equal(readFileSync(fixture, 'utf8'), output, 'the fixture is reproducible');
const shape = node => {
  if (node instanceof Y.XmlText) return { kind: 'text', runs: node.toDelta() };
  if (node instanceof Y.XmlElement) return {
    kind: node.nodeName, attrs: node.getAttributes(), children: node.toArray().map(shape),
  };
  return { kind: 'fragment', children: node.toArray().map(shape) };
};
const rustAt = process.argv.indexOf('--rust-state');
if (rustAt >= 0) {
  const made = JSON.parse(readFileSync(process.argv[rustAt + 1], 'utf8'));
  assert.equal(made.length, result.length);
  for (let index = 0; index < made.length; index++) {
    const ours = made[index], expected = result[index];
    assert.equal(ours.name, expected.name);
    const source = new Y.Doc(); source.clientID = 8;
    Y.applyUpdate(source, Buffer.from(ours.update, 'base64'));
    assert.deepEqual(read(source), expected.json, `${ours.name}: Rust to the binding`);
    assert.deepEqual(JSON.parse(JSON.stringify(shape(source.getXmlFragment('default')))),
      JSON.parse(JSON.stringify(shape(create(expected.body, 1).getXmlFragment('default')))), `${ours.name}: shared XML shape`);
  }
}
console.log(`${result.length} binding correctness vectors verified`);
