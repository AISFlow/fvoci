#!/usr/bin/env node
/**
 * Dev-only: pinned Yjs 13.6.32 + Tiptap y-tiptap XML fragment "prosemirror"
 * exchanged through the Yrs updateV1 bridge. Product code must never import this.
 */
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { getSchema } from "@tiptap/core";
import StarterKit from "@tiptap/starter-kit";
import {
  prosemirrorJSONToYXmlFragment,
  yDocToProsemirrorJSON,
} from "@tiptap/y-tiptap";
import * as Y from "yjs";

const FRAGMENT = "prosemirror";
const here = path.dirname(fileURLToPath(import.meta.url));
const bridgeBin =
  process.env.YRS_BRIDGE ??
  path.join(here, "..", "target", "debug", "yrs-bridge");

function b64(u8) {
  return Buffer.from(u8).toString("base64");
}

function fromB64(s) {
  return new Uint8Array(Buffer.from(s, "base64"));
}

function seedDoc(text, clientID) {
  const doc = new Y.Doc({ gc: false });
  if (clientID !== undefined) doc.clientID = clientID;
  const schema = getSchema([StarterKit]);
  prosemirrorJSONToYXmlFragment(
    schema,
    {
      type: "doc",
      content: [
        {
          type: "paragraph",
          content: [{ type: "text", text }],
        },
      ],
    },
    doc.getXmlFragment(FRAGMENT),
  );
  return { doc, schema };
}

function jsonOf(doc) {
  return yDocToProsemirrorJSON(doc, FRAGMENT);
}

function textOf(doc) {
  return JSON.stringify(jsonOf(doc));
}

function insertParagraph(doc, text) {
  const frag = doc.getXmlFragment(FRAGMENT);
  const el = new Y.XmlElement("paragraph");
  const t = new Y.XmlText();
  t.insert(0, text);
  el.insert(0, [t]);
  frag.insert(frag.length, [el]);
}

class Bridge {
  constructor(child) {
    this.child = child;
    this.rl = createInterface({ input: child.stdout });
    this.queue = [];
    this.pending = [];
    this.rl.on("line", (line) => {
      const resolve = this.pending.shift();
      if (resolve) resolve(JSON.parse(line));
      else this.queue.push(JSON.parse(line));
    });
  }

  request(body) {
    return new Promise((resolve, reject) => {
      this.pending.push(resolve);
      this.child.stdin.write(`${JSON.stringify(body)}\n`, (err) => {
        if (err) reject(err);
      });
    });
  }

  async close() {
    this.child.stdin.end();
    await new Promise((r) => this.child.on("close", r));
    this.rl.close();
  }
}

async function openBridge() {
  const child = spawn(bridgeBin, [], {
    stdio: ["pipe", "pipe", "inherit"],
  });
  const br = new Bridge(child);
  const ping = await br.request({ cmd: "ping" });
  if (!ping.ok) throw new Error(`yrs-bridge ping failed: ${JSON.stringify(ping)}`);
  return br;
}

function assert(cond, msg) {
  if (!cond) throw new Error(msg);
}

async function run() {
  const results = [];
  const br = await openBridge();
  try {
    const ping = await br.request({ cmd: "ping" });
    results.push({
      name: "pins-and-gc",
      ok:
        ping.skip_gc === true &&
        ping.fragment === FRAGMENT &&
        ping.encoding === "updateV1",
      detail: ping,
    });

    // 1. Tiptap JSON -> Yjs updateV1 -> Yrs -> Yjs, Korean
    await br.request({ cmd: "reset" });
    const { doc: a, schema } = seedDoc("안녕", 1);
    assert(a.gc === false, "Y.Doc gc must be false");
    const uA = Y.encodeStateAsUpdate(a);
    const applied = await br.request({ cmd: "apply_v1", b64: b64(uA) });
    assert(applied.ok, `apply: ${applied.error}`);
    const state1 = await br.request({ cmd: "encode_state_v1" });
    const back = new Y.Doc({ gc: false });
    Y.applyUpdate(back, fromB64(state1.b64));
    const t1 = textOf(back);
    results.push({
      name: "tiptap-korean-roundtrip-via-yrs",
      ok: t1.includes("안녕"),
      detail: t1,
    });

    // 2. Concurrent edits: Korean + emoji, different clientIDs, merge in Yrs
    await br.request({ cmd: "reset" });
    const { doc: c1, schema: s1 } = seedDoc("가나다", 11);
    const { doc: c2, schema: s2 } = seedDoc("🚀✨", 22);
    await br.request({ cmd: "apply_v1", b64: b64(Y.encodeStateAsUpdate(c1)) });
    await br.request({ cmd: "apply_v1", b64: b64(Y.encodeStateAsUpdate(c2)) });
    const merged = await br.request({ cmd: "encode_state_v1" });
    const mdoc = new Y.Doc({ gc: false });
    Y.applyUpdate(mdoc, fromB64(merged.b64));
    const mt = textOf(mdoc);
    results.push({
      name: "concurrent-korean-emoji-via-yrs",
      ok: mt.includes("가나다") && mt.includes("🚀✨"),
      detail: mt,
    });

    // 3. Duplicate + out-of-order updates
    await br.request({ cmd: "reset" });
    const seq = new Y.Doc({ gc: false });
    seq.clientID = 7;
    const schemaSeq = getSchema([StarterKit]);
    prosemirrorJSONToYXmlFragment(
      schemaSeq,
      {
        type: "doc",
        content: [
          {
            type: "paragraph",
            content: [{ type: "text", text: "one" }],
          },
        ],
      },
      seq.getXmlFragment(FRAGMENT),
    );
    const upd1 = Y.encodeStateAsUpdate(seq);
    seq.transact(() => {
      insertParagraph(seq, "two한글");
    });
    const upd2 = Y.encodeStateAsUpdate(seq, Y.encodeStateVectorFromUpdate(upd1));
    // out of order: upd2 then upd1 then duplicate upd2
    await br.request({ cmd: "reset" });
    const o2 = await br.request({ cmd: "apply_v1", b64: b64(upd2) });
    const o1 = await br.request({ cmd: "apply_v1", b64: b64(upd1) });
    const o2d = await br.request({ cmd: "apply_v1", b64: b64(upd2) });
    const afterOo = await br.request({ cmd: "encode_state_v1" });
    const oo = new Y.Doc({ gc: false });
    Y.applyUpdate(oo, fromB64(afterOo.b64));
    const ot = textOf(oo);
    const inOrder = new Y.Doc({ gc: false });
    Y.applyUpdate(inOrder, upd1);
    Y.applyUpdate(inOrder, upd2);
    Y.applyUpdate(inOrder, upd2);
    results.push({
      name: "duplicate-out-of-order-updates",
      ok:
        o2.ok &&
        o1.ok &&
        o2d.ok &&
        ot.includes("one") &&
        ot.includes("two한글") &&
        textOf(inOrder) === ot,
      detail: { ot, inOrder: textOf(inOrder) },
    });

    // 4. State-vector reconnection (client SV -> Yrs diff -> apply)
    await br.request({ cmd: "reset" });
    const live = new Y.Doc({ gc: false });
    live.clientID = 3;
    const schemaLive = getSchema([StarterKit]);
    prosemirrorJSONToYXmlFragment(
      schemaLive,
      {
        type: "doc",
        content: [
          {
            type: "paragraph",
            content: [{ type: "text", text: "before" }],
          },
        ],
      },
      live.getXmlFragment(FRAGMENT),
    );
    await br.request({
      cmd: "apply_v1",
      b64: b64(Y.encodeStateAsUpdate(live)),
    });
    const clientSv = Y.encodeStateVector(live);
    insertParagraph(live, "offline한글");
    // server also got a different edit from another client
    const other = new Y.Doc({ gc: false });
    other.clientID = 4;
    Y.applyUpdate(other, Y.encodeStateAsUpdate(live)); // includes offline? wait no - live already has offline
    // Redo: snapshot client before offline edit
    await br.request({ cmd: "reset" });
    const server = new Y.Doc({ gc: false });
    server.clientID = 99;
    const schemaS = getSchema([StarterKit]);
    prosemirrorJSONToYXmlFragment(
      schemaS,
      {
        type: "doc",
        content: [
          {
            type: "paragraph",
            content: [{ type: "text", text: "base" }],
          },
        ],
      },
      server.getXmlFragment(FRAGMENT),
    );
    await br.request({
      cmd: "apply_v1",
      b64: b64(Y.encodeStateAsUpdate(server)),
    });
    const disconnected = new Y.Doc({ gc: false });
    disconnected.clientID = 5;
    Y.applyUpdate(disconnected, Y.encodeStateAsUpdate(server));
    const svBefore = Y.encodeStateVector(disconnected);
    insertParagraph(disconnected, "클라한글");
    // server-side edit while disconnected
    const extra = new Y.Doc({ gc: false });
    extra.clientID = 6;
    Y.applyUpdate(extra, Y.encodeStateAsUpdate(server));
    insertParagraph(extra, "서버emoji🎉");
    await br.request({
      cmd: "apply_v1",
      b64: b64(Y.encodeStateAsUpdate(extra)),
    });
    // client reconnects: send SV, get diff, also send own offline update
    const diff = await br.request({ cmd: "diff_v1", sv_b64: b64(svBefore) });
    assert(diff.ok, `diff: ${diff.error}`);
    Y.applyUpdate(disconnected, fromB64(diff.b64));
    await br.request({
      cmd: "apply_v1",
      b64: b64(Y.encodeStateAsUpdate(disconnected)),
    });
    const afterRe = await br.request({ cmd: "encode_state_v1" });
    const reDoc = new Y.Doc({ gc: false });
    Y.applyUpdate(reDoc, fromB64(afterRe.b64));
    const rt = textOf(reDoc);
    results.push({
      name: "state-vector-reconnect",
      ok: rt.includes("base") && rt.includes("클라한글") && rt.includes("서버emoji🎉"),
      detail: rt,
      unused_sv: Buffer.from(clientSv).length,
    });

    // 5. Persistence byte roundtrip + subsequent edit
    const persisted = await br.request({ cmd: "encode_state_v1" });
    await br.request({ cmd: "reset" });
    const loaded = await br.request({
      cmd: "apply_v1",
      b64: persisted.b64,
    });
    const inspect = await br.request({ cmd: "inspect" });
    const afterLoad = new Y.Doc({ gc: false });
    Y.applyUpdate(afterLoad, fromB64(persisted.b64));
    insertParagraph(afterLoad, "after-persist");
    const post = await br.request({
      cmd: "apply_v1",
      b64: b64(Y.encodeStateAsUpdate(afterLoad)),
    });
    const finalState = await br.request({ cmd: "encode_state_v1" });
    const finalDoc = new Y.Doc({ gc: false });
    Y.applyUpdate(finalDoc, fromB64(finalState.b64));
    const ft = textOf(finalDoc);
    results.push({
      name: "persist-bytes-then-edit",
      ok:
        loaded.ok &&
        post.ok &&
        inspect.skip_gc === true &&
        inspect.fragment === FRAGMENT &&
        ft.includes("after-persist") &&
        ft.includes("base"),
      detail: { inspect, ft, persistedBytes: fromB64(persisted.b64).byteLength },
    });

    const failed = results.filter((r) => !r.ok);
    const summary = {
      ok: failed.length === 0,
      yjs: "13.6.32",
      fragment: FRAGMENT,
      encoding: "updateV1",
      gc: false,
      yrs_bridge: bridgeBin,
      results,
    };
    console.log(JSON.stringify(summary, null, 2));
    if (failed.length) process.exit(1);
  } finally {
    await br.close();
  }
}

run().catch((err) => {
  console.error(err);
  process.exit(1);
});
