import { expect,test } from "bun:test";
import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import ts from "typescript";
import * as Vue from "vue";
import * as Y from "yjs";
import { tiptapJsonToYDoc,yDocToTiptapJson } from "@fvoci/editor/collab-tiptap";
import { OffWikiDraft,decodeUpdate,encodeUpdate,loadBody,ownerKey } from "../../features/documents/off-wiki-draft";
import type { OffWikiOwner } from "../../features/documents/off-wiki-draft";
import type { VersionedBody } from "../../features/documents/versioned-body-api";
const raw=readFileSync(new URL("./useOffWikiBody.ts",import.meta.url),"utf8").replace(/^import[\s\S]*?from ["'][^"']+["'];\s*/gm,"").replace("export function","function");
const script=ts.transpile(raw,{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.None});
class ProblemError extends Error {constructor(readonly status:number){super(String(status))}}
const owner={actorId:"A",credentialId:"credential-A",workspaceId:"workspace",targetId:"target"};
function body(scope:OffWikiOwner,text:string,tailSeq:string):VersionedBody{
 const doc=tiptapJsonToYDoc({type:"doc",content:[{type:"paragraph",attrs:{id:"stable-block"},content:[{type:"text",text}]}]});
 const source={targetId:scope.targetId,tailSeq,snapshotV1:encodeUpdate(Y.encodeStateAsUpdate(doc)),tailV1:[],contentJson:yDocToTiptapJson(doc),writable:true};doc.destroy();return source;
}
function harness(storageKind:"null"|"access-denied"|"write-denied"){
 const scope=Vue.shallowRef<OffWikiOwner>({...owner});const reads:{scope:OffWikiOwner,resolve:(value:VersionedBody)=>void}[]=[];
 const stored=new Map<string,string>();
 const windowObject={
  get sessionStorage(){if(storageKind==="access-denied")throw new Error("unavailable");if(storageKind==="null")return null;return{getItem:(key:string)=>stored.get(key)??null,setItem:()=>{throw new Error("quota")},removeItem:(key:string)=>stored.delete(key)}},
  addEventListener(){},removeEventListener(){},
 };
 const factory=runInNewContext(`${script}\nuseOffWikiBody`,{...Vue,Y,OffWikiDraft,decodeUpdate,encodeUpdate,loadBody,ownerKey,ProblemError,sourceDraftAuthRetiredKey:Symbol(),inject:()=>Vue.ref(false),window:windowObject,AbortController,
  readVersionedBody:()=>new Promise<VersionedBody>(resolve=>reads.push({scope:{...scope.value},resolve})),
  saveVersionedBody:()=>{throw new Error("refresh must not perform a write")},
  createDocumentFromDraft:()=>{throw new Error("refresh must not create a document")},
 }) as typeof import("./useOffWikiBody").useOffWikiBody;
 const effect=Vue.effectScope();const current=effect.run(()=>factory(()=>scope.value,()=>true))!;
 return{scope,reads,current,effect};
}
async function settle(){await Promise.resolve();await Vue.nextTick();await Promise.resolve()}
function edit(doc:Y.Doc){const paragraph=doc.getXmlFragment("prosemirror").get(0) as Y.XmlElement;(paragraph.get(0) as Y.XmlText).insert(0,"late private ")}
for(const storageKind of ["null","access-denied","write-denied"] as const){
 test(`confirmed restore refresh preserves late live edits with ${storageKind} storage and records conflict`,async()=>{
  const h=harness(storageKind);h.reads[0]!.resolve(body(owner,"start","1"));await settle();
  const original=h.current.draft.value!;const generation=h.current.generation.value;
  const refresh=h.current.load();edit(original.doc);
  h.reads[1]!.resolve(body(owner,"confirmed restored","2"));await refresh;
  expect(h.current.draft.value).toBe(original);
  expect(h.current.generation.value).toBe(generation);
  expect(JSON.stringify(original.mine)).toContain("late private");
  expect(original.start.tailSeq).toBe("1");
  expect(original.latest?.tailSeq).toBe("2");
  expect(JSON.stringify(original.comparison?.current)).toContain("confirmed restored");
  h.effect.stop();
 });
}
test("a healthy confirmed refresh replaces only the still-clean owned native draft",async()=>{
 const h=harness("null");h.reads[0]!.resolve(body(owner,"start","1"));await settle();const original=h.current.draft.value;
 const refresh=h.current.load();h.reads[1]!.resolve(body(owner,"confirmed restored","2"));await refresh;
 expect(h.current.draft.value).not.toBe(original);expect(original?.active).toBe(false);
 expect(h.current.draft.value?.start.tailSeq).toBe("2");expect(JSON.stringify(h.current.draft.value?.mine)).toContain("confirmed restored");h.effect.stop();
});
test("a same-owner read-only refresh updates reactive authority without discarding dirty mine",async()=>{
 const h=harness("null");h.reads[0]!.resolve(body(owner,"start","1"));await settle();const original=h.current.draft.value!;edit(original.doc);
 const editable=Vue.computed(()=>h.current.writable.value);
 expect(editable.value).toBe(true);
 const refresh=h.current.load();h.reads[1]!.resolve({...body(owner,"start","1"),writable:false});await refresh;
 expect(h.current.draft.value).toBe(original);expect(editable.value).toBe(false);
 expect(JSON.stringify(original.mine)).toContain("late private");
 expect(await h.current.save()).toBe(false);h.effect.stop();
});
test("source buffer and an unknown save command survive refresh without receipt inference",async()=>{
 const h=harness("write-denied");h.reads[0]!.resolve(body(owner,"start","1"));await settle();const original=h.current.draft.value!;edit(original.doc);
 original.setSourceBuffer({text:"unapplied Markdown",baseV1:Y.encodeStateAsUpdate(original.doc)});
 await expect(original.save(async()=>{throw new Error("response lost")})).rejects.toThrow();const command=original.frozen;
 const refresh=h.current.load();h.reads[1]!.resolve(body(owner,"confirmed other update","2"));await refresh;
 expect(h.current.draft.value).toBe(original);expect(original.sourceBuffer?.text).toBe("unapplied Markdown");expect(original.frozen).toBe(command);expect(original.latest?.tailSeq).toBe("2");h.effect.stop();
});
test("refresh A->B->A cannot reattach retired owner or overwrite the newest private draft",async()=>{
 const h=harness("null");h.reads[0]!.resolve(body(owner,"start","1"));await settle();const original=h.current.draft.value!;edit(original.doc);
 const oldRefresh=h.current.load();h.scope.value={...owner,actorId:"B",credentialId:"B"};h.scope.value={...owner};
 const newest=h.reads.at(-1)!;newest.resolve(body(newest.scope,"newest A","4"));await settle();const fresh=h.current.draft.value!;edit(fresh.doc);
 h.reads[1]!.resolve(body(owner,"old restore response","2"));await oldRefresh;
 expect(h.current.draft.value).toBe(fresh);expect(JSON.stringify(fresh.mine)).toContain("newest A");expect(JSON.stringify(fresh.mine)).not.toContain("old restore response");expect(original.active).toBe(false);h.effect.stop();
});
