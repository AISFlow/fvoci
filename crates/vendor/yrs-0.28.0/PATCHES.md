# FVOCI source patch to Yrs 0.28.0

This is a FVOCI-owned, same-version, source-pinned patch. It is not an
upstream accepted patch or a separately maintained upstream release.
The original crate archive SHA256 is
`52c70dc8beca8666c77612a96889106ca3cd65318609721f464624ff79685da9`;
that checksum does **not** identify this patched package.

## Production delta

- `src/retained.rs`: bounded read-only borrowed views of retained store blocks,
  roots and decoded Update items, without parsing, integration or serialization.
- `src/transaction.rs`: a read-only `ReadTxn::visit_retained` adapter.
- `src/lib.rs`: public module registration.
- `src/block_store.rs`: crate-private client table capacity observation.

At the original packaging freeze, the four production files matched the
independently reviewed frozen v4 prototype. Later deltas below have their own
source and validation boundaries.
No decoder, integrator, GC, native representation, protocol, limits, version or
feature change is included. The accessor does not prove an archive complete;
FVOCI's archive consumer and real restore acceptance remain pending.

## Tests and licensing

The two new test files are the separately unreviewed v5 witness tests. They
retain exposed metadata, reject equal-payload structural duplicates, disclose
split/unknown structure as unproved, and check known unavailable intervals.
Their focused compile/run acceptance remains pending at this packaging freeze.
All 68 files of the pinned cached crate source are retained; the MIT license notice
omitted from that package is included from its pinned upstream revision.
`PROVENANCE.json` records the original and changed file hashes, source revision,
license source and precise acceptance boundaries.

## Security update reconciliation

The FVOCI coordinator owns reconciliation against upstream Yrs maintenance and
security changes. For each upstream update, compare all changed production files,
carry only the required accessor delta, preserve the original version/features
and native contracts unless a separate change is approved, and obtain fresh
fixed-source review plus focused and product compatibility/restore validation.
An absence of known advisories is not a security audit. Do not silently upgrade,
claim upstream acceptance, or treat this original archive checksum as a checksum
of the local patched source. The global registry cache must remain unmodified.

## Pending archive snapshot capacity delta

After separately accepted a85 package/source and its actual focused controls,
`src/state_vector.rs` adds only `StateVector::table_capacity`, delegating to
standard `HashMap::capacity`, plus two getter/literal-snapshot controls. It allows
the archive consumer to bound sparse SV iteration after standard snapshot decode.
It does not parse, resolve, integrate, encode or mutate native data; the original
four v4 production files were unchanged at the capacity-only freeze. Its two
focused controls passed in the author v2 run; fixed source review accepted the
consumer remediation through0189 only. This is not product archive acceptance. Its current source hash is separately
recorded in `pending_archive_snapshot_capacity` in PROVENANCE.json. IdSet already
uses an ordered tree map; no capacity getter or IdSet change is included. Existing
child memory/deadline limits govern standard decode allocations; this getter is
not a claim of parser pre-allocation enforcement. Fresh fixed review and affected
actual validation are required for later deltas and product adoption.

## Pending archive snapshot materialization delta

At parent `0189c57d9bf270ea8a5f9cbb5514e5d8a0e58ff4`, actual consumer tests
passed14 and failed1: the independent saved cut expected `A` but standard
read-transaction encoding returned `A한글🙂`. The remaining full-worker/root
commands did not run. Existing prepared proof15 demonstrated that physical
snapshot boundaries must be materialized and encoded before the same mutable
transaction drops and normal cleanup remerges them.

`src/transaction.rs` now exposes only `TransactionMut::materialize_snapshot`,
a documented thin delegation to the unchanged existing `split_by_snapshot`.
No original production byte is otherwise changed. Callers must use a separate
scratch document, encode within the same live mutable transaction, and prove
all resulting retained intervals and exact standard SV/DS; a half-surrogate cut
can still produce excess and must fail explicitly. The appended two focused
tests declare literal snapshot bytes and independent expected text, Unicode,
deletions and a second root, source immutability, after-drop remerge and
surrogate-excess controls. These new tests and this adapter are NOT RUN and
await separate fixed-source review and compile START. No standard splitting,
UTF16, parser, encoder, integration, GC, native format, version, features,
manifest or lock implementation is copied or changed. Ordinary restore remains
unchanged; only the archive scratch caller uses the new adapter.

Exact pending source hash, parent and validation boundary are recorded in
`pending_archive_snapshot_materialization` in `PROVENANCE.json`. Previous
source-only review and getter tests do not accept this new adapter, historical
reconstruction, normalized/inherited-item completeness, consent, paired native
restore or full W7. The global registry cache remains untouched.

## Pending test-only owned normalization proof after45e

The fixed DS correction at `45e15e914353ce515fc6a97469c00ed8109a3cfc`
has separate scoped independent acceptance and actual focused2/full-worker42/
root-check/root-policy9 PASS. Those accepted production bytes remain unchanged.
The earlier pending sections preserve their historical packaging-freeze state;
they are not a statement that the later recorded batches did not execute.

This candidate appends only a `cfg(test)` child module to `src/update.rs`.
Removing the exact appended bytes recovers the complete45e source, including
the unchanged private `Update::missing_dependency` body/visibility. No public
helper, codec, integrator, ordinary Load/Apply/restore, consumer, dependency,
manifest or lock change is included. `PROVENANCE.json` binds prefix/append/current
hashes and the eight exact test names. Compilation/runtime is NOT RUN; rustfmt
parse alone is not type or runtime validation. Separate START and independent
source/oracle review are required before any proposed public adoption.

The test candidate captures owned copies before consuming the same decoded
Update once; rejects foreign Branch/subdocument/unsupported type pointers; uses
only existing standard materialize/splice on unresolved owned boxes and one
complete native scratch clone with one held comparison transaction; then uses
the unchanged SDK resolver. It never splices boxes after their pointers resolve
into scratch and drops them before that transaction/document. Identity proof
compares exact ID/range, resolved owner/key, origins and typed content, with
full interval coverage and semantic DS checks. Independent Unicode, paragraph,
marks, deletes, saved-cut and losing-map literals plus forged metadata,
GC/loss/Skip/pending/surrogate/budget controls are SOURCE expectations only.
Counters are conservative abstract test work/copy charges, not production
allocator or realistic8/32MiB resource proof. Source complete bytes/SV/DS/IDs
remain asserted unchanged.

This prototype does not accept the native product archive, consent flow, real
two-installation restore/RLS/history/files/revision/peer-edit tracer, full W7
models or operator drill. Those remain mandatory under the existing paired plan.

### Test-only prototype compile correction after53f

The exact first focused SDK command at53f exited101 with three compile errors;
all eight test bodies were NOT RUN and no test binary launched. Its immutable
log/time/metadata/source receipts remain separate from this candidate. The
appendix now corrects only two IdRangeReader loop patterns and uses the existing
`ItemPtr::from(&mut *item)` mutable Box reborrow before standard splice. The
Type cloned-box ownership assertion now uses `std::ptr::eq`, because ItemPtr Eq
compares native IDs rather than allocation addresses. No literal, counter or
expected-result relaxation is included. Compilation/runtime at the new source
hash is NOT RUN and needs a new explicit START; source/oracle review remains
independent. The production prefix and every public/consumer/ordinary path
remain unchanged; original failure is preserved.

### Test-only prototype XmlText/TextRef oracle contract correction after1786

The1786 focused command compiled and ran7PASS/1FAIL; paragraph failed its
preliminary getter assertion before matcher/native/history proof ran. XmlText
GetString includes formatting tags (locked public example/renderer), unlike
TextRef GetString. The corrected oracle preserves the SAME plain Unicode
literal through the existing standard XmlTextRef AsRef<TextRef> view, separately
asserts exact `<bold>한글</bold>🙂 끝🧪` XML, and checks independent native
Format10:11 bold=true/10:12 bold=null BEFORE matcher output. Original failures
at53f/1786 remain immutable. No custom conversion/unsafe, matcher/counter/
coverage/savedcut or seven passing control change is included. Compile/runtime
at this new hash is NOT RUN and needs a separate START plus independent oracle
review. P2 raw Skip/SV0 omitted-ledger coverage remains OPEN and unmodified;
these tests cannot accept production/public helper or W7 product scope.

### Original-negative actual held Skip/SV0 coverage tests after57ba

The57ba focused eight actual tests PASS, including both text contracts/native
mark/deleted-A/types/savedcut controls; F2 remains independent and OPEN. This
source-only candidate appends ONLY two original-negative tests and their narrow
standard-API fixture under the existing cfg(test) child. Removing the exact
added section recovers the FULL57ba update.rs bytes: capture/run/verify/budget/
matcher/eight prior test bodies and every production byte stay unchanged.
The ordinary SDK fixture independently declares map10:0a=A/10:1b=B before
writer output, standard public suffix atSV10:1 and fresh apply yields actual
Skip0len1+availableItem1/rootmetadata Map/keyb/valueB, SV0/pendingfalse/emptyDS.
Direct-held and standardclone omitted-ledger proofs both require rejection;
raw typed census/bytes/SV/DS immutability run BEFORE rejection assertions.
The clone callback records actual rows if original verifier returnsOk, without
changing that verifier. Two diagnostic fixture prints allow actual incidental
errors to be distinguished from false acceptance under focused --nocapture.
Actual new2 COMPILE/RUNTIME NOT RUN; RED is a prediction, not observed proof.
A later minimumcoveragefix/fullten regression/fresh independent review remains
required; no source/publicSDK/API/DB/native product/fullW7 adoption is implied.

### Original-negative fixture trait-scope correction after7ad

The originalnew2 command at7ad compileFAILED E0599; both bodies NOT RUN, no
actual falseacceptance observed. Vendor manifests use Rust2018; the standard
StateVector::from_iter needs FromIterator in scope. This candidate adds ONLY
local `use std::iter::FromIterator;` inside the NEW standard fixture, plus
precise metadata. Removing that single statement restores7ad update.rs bytes,
and removing the new fixture/tests restores the full57ba source. Both rejection
oracles/literals/methods/all old helpers/eight tests/production prefix remain
unchanged. Original failure preserved; new compile/runtime NOT RUN and needs
new explicit START before any RED claim or later semanticfix.

### Nonzero-prefix original controls after observed SV0 incidental rejection

At actual007 the two originalSV0 controls PASS with Err(missing-coverage),
not predicted falseacceptance: a zero-range client entry survives structural
IdSet equality. Preserve that contraryactualevidence and earlier staticreview.
The nocapture evidence-wrapper parser failed afterSDKexit0; savedlog/time plus
actualsource/binary/d/features receipts recovered READONLY, no Cargo rerun.
This source-only candidate appends ONLY two nonzero-prefix originals +narrow
ordinary SDK fixture. Removing exactaddition recovers FULL007 includingall10
prior test bodies, rawcensus/helpers/matcher/budget and everyproductionbyte.
Independent native A0baseline captured/applied once, bB1/cC2 standard suffix
atSV2 appliedonce withoutledger, expected actualA0+Skip1+C2/SV1/noPending/
emptyDS must be asserted beforeproof. CapturedA-only directheld/standardclone
both require whole incomplete rejection; actualrawcensus/bytes/SV/DS immutable
BEFORE rejectionassertion and clone actualrows observed if verifier returnsOk.
No synthesizedSkip/parser/clocktweak/reseed or semanticguardfix included.
Newdiagnostics use explicitformatarguments; old2warnings/bodies unchanged.
Actualnew2 COMPILE/RUNTIME NOT RUN, broader falseacceptance stillHYPOTHESIS.
Separate exactSTART and originalresult precede anyminimumfix/full12/freshreview;
no publicSDK/engine/product/consent/paired/fullW7 adoption implied.

### Test-only minimum actual held raw coverage correction after061926

Actual original nonzero2 at061926 compiled0PASS/2FAIL/422filtered. Both
unchanged proof paths incorrectly returnedOk/comparisons1 despite literal
actualA0+Skip1+C2/SV1/noPending/emptyDS/capturedA-only; the standard clone
callback observed exactly those same3rows. Canonicalbytes/SV/DS/rawcensus
immutability assertions ran before rejection failure. Original log/time/
source/binary/d/features receipts remain immutable; earlierSV0actualPASS
incidentalErrmissing-coverage remains contrary evidence to its staticprediction.

Only the existing cfg(test) verify now scans EVERY actual held raw block BEFORE
materialization. It charges traversal/ranges, refuses Skip/GC/actualDeleted
content, invalidclient/zero/overflow/UTF16length, and builds standardIdSet
required from exact available ID/ranges including any beyondcontiguousSV.
IndependentSV consistency and comparedledger coverage must both match.
Available deleted-content Items are preserved; deletionFLAG is not unavailability.
All12 complete testbodies/literals/counters/fixtures and capture/incoming/run/
matcher/budget/production66090prefix stay unchanged. No parser/customrange
implementation/storewrite/publichelper/ordinarypath/adoption is included.
Current full12 compile/runtime NOTRUN pending separate START and new fixed
independent Sol review including the getteroracle correction/sourcechain.
This minimum held-store fix does not establish sourcecanonical-beforeclone
completeness, general clonefaithfulness/lifetime/sanitizer/realcost/product
pairedrestore/fullW7; those remain mandatory promotion/acceptance boundaries.

### Pending actual owned archive witness SDK/consumer source after accepted8dd

Root sourceactivation msg_eb1548a7fbfe follows actual independent8dd
ACCEPT_THIS_TEST_PROTOTYPE_CORRECTION_ONLY and actual12PASS. All12 complete
prototype bodies/fixtures/counters/helper bytes remain unchanged. Update.rs
changes ONLY private resolver visibility to pub(crate), with body/algorithm
unchanged; reversing that token recovers the FULL8dd source and original
66090-byte production prefix. Retained borrowed-view behavior stays unchanged;
new child witness.rs provides opaque owned capture and scratch proof, with
minimal exports/docs. No block/store/slice/decoder/encoder/integrator/GC/
ordinaryLoad/Apply/Restore/dependency/manifest/lock change is included.

The new actual helper holds the SAME canonicalReadTxn across actualALLraw
interval/SV/DS/root kinds and ownedcanonical witnesses BEFOREstandardencode
and oneclone. Scratch actualcensus mustmatch independently; canonicaltyped
witnesses then originaldecodedinput witnesses compare in SAMEheldscratchTX
using maintained unresolvedsplice/materialize/resolver. Allboxes dropbefore
TX/Doc; no inputsplice afterresolution, no foreignBranch/tokenborrowescape.
StandardType content clone creates fresh Branch/type, not source subtree.
Precharge checkedconservative header/content bounds, standardserde exactJSON
counting, buffers/structures/Vecgrowth BEFOREdelegatedallocation; actualnative
len/capacity guard follows standardEncoderV1. This is accounting plus original
process caps, NOT allocator/instruction sandbox or measurednativecost.

Consumer availablepayload duplicate copies/exactunsplit matcher replaced
by opaque witnesses; proof precedes readonlycanonical classifier, loss metadata/
reversecoverage/schema/reference/ancestry/conflict guards retained. Refused
proof adds explicit diagnostics and cannot yield Complete. OldGCmarker test
now requires incomplete nativeidentity proof while retaining exactunavailable
and forward/reverse controls; normalized baretext positive tests the new proof
without claiming schema/userarchive acceptance. Ordinaryparagraph+Unicode/
Format/deletedavailable/history source regression added. Eight new publichelper
controls cover literals, inheritedlosingmap/replay, forgedowner/key/origins/DS,
realSkip sourcebeforeclone, sameIDchangedclone, JSONescaping/bounds, loss/foreign
parent/budget and sparse/deep/replayquota. COMPILE/RUNTIME NOTRUN; predictions
are not evidence. Separate START/freshfixedSolreview/realchildcost required.
Typedconsent/pairedUI→RustAPI→restrictedDB→freshclient/history/files/newedit/
fullW7models/operator/full0.5 remain mandatory and unaccepted.

### R1/R2 original test-first controls after REQUEST_CHANGES (source only)

Fixed3e public review2c267392 REQUEST_CHANGES for before-materialize content
copies/scans/Vec shifts/growth and before-scratch Doc/txn/root structural
reservations. Actual3e SDK20/worker43 PASS remain bounded regressions; original
SDK receipt wrapper1 and separate read-only recovery remain immutable.

This successor changes ONLY cfg(test) effect markers at actual SDK delegation
boundaries plus six new test functions/focused fixture helpers in witness.rs.
Removing the markers/module/additions recovers the ENTIRE3e witness source;
all old20 bodies/oracles and all production algorithms remain unchanged.
Literal ordinary standard writer -> maintained unresolved fragmentation ->
standard encode/decode -> capture sameUpdate -> applyONCE fixtures cover
1024*64 ASCII String fragments, Korean/emoji Any vectors, 512 following map
blocks and 64 empty roots plus four root kinds. Callback refusal must precede
materialize/splice/scratch effects; source bytes/SV/DS remain unchanged.
Positive controls are present after each refusal (NOTRUN if first refusal
fails); no successful whole request, allocation overrun or nativecost inferred.

JSON direct Item split control is explicitly not a successful codec roundtrip:
ordinary Any decode supplies legal root/IDs, then content is explicitly changed
to maintained legacy JSON and captured before applying once. A separate original
N=3 standard JSON encode/decode test EXPECTS success/literal values; source N+1
reader discrepancy is pending actual reproduction, no assertErr-to-green,
skip/customcodec/parser/defaultSDK edit or support exclusion. R1/R2 fixes are
NOT implemented in this test-first candidate. All six COMPILE/RUNTIME NOTRUN;
register exact names/frozen commands and obtain separate START before Cargo.
Native parent/cost/paired UI-API-restrictedDB-history-files-peer/fault/current
allmodels/operator/fullW7 acceptance remain mandatory; no worker_done.

### Minimum R1/R2 correction after actual original5 RED (source, NOTRUN)

Source fixes only witness.rs and five-line ClientBlockList capacity accessor,
plus metadata. Standard materialize/splice/lookup/integrate/codec/default
algorithms and quotas stay unchanged; all previous26 tests/fixtures are
byte-identical. Actual original5 all reached intended Budget assertions and
wrongly Ok: String inspected131072/effects3071 calls, Any/JSON/shift effects1,
empty64 roots owned7856/scratch1. Those source/library accounting failures do
not claim physical allocator overrun or genuine product archive acceptance.

R1 uses actual client list count/capacity, checked all-client scratch growth,
full content-copy/UTF16 scans/Item-parent-key and vector movement/growth debits
BEFORE standard materialize/unresolved splice. Linked materialization refuses
before standard linked_by cloning. Growth bound is explicitly pinned Rust
48a229cea RawVec/Vec/Global source, not a portable doubling guarantee. New
actual-capacity regression fills observed capacity, refuses allocation/shift
before effect, then allows the SAME operation and verifies actual capacity,
count, literal IDs/content and source immutability.

R2 separately reserves base StoreInner/Options/Doc/transactions, boxed roots
and root HashMap structural layouts, decoder/store row vectors/Items, parent
maps/change sets/range nodes and payload containers BEFORE creating scratch
Doc/roots/decoder. Pinned Rust/std hashbrown0.17.1 and BTree source layout
bounds are conservative logical requested-layout accounting, not physical
heap metadata/RSS/instruction sandbox. No structural bound rests solely on
wire bytes; unchanged cap/standard wire encode bound remains separate.

Fixed source compile/runtime/new independentreview/native childcost NOTRUN.
Separate original JSON codec-success oracle still NOTRUN; direct JSON split
control does not establish a successful wire/product roundtrip. Preserve all
original logs/receipts and initial SDKwrapper1; no tests skipped or assertions
converted to expect errors. Sourcepositive SDK/worker/parent/nativecost and
real paired UI/API/restrictedDB/history/files/peer/fault/allcurrentmodels/full
W7 remain required before adoption/completion, no worker_done.

### Aligned String R1 test-first successor after first compile failure

Historical2824 SDK6 compilation failed E0603/E0107 on private generic IdRanges
and E0277 on new capacity positive String conversion; six bodies NOTRUN.
Only compile repairs use actual backing crate::ids::IdRanges<()> and installed
SplittableString From<&str>. Previous27 controls are unchanged except this
exact constructor repair; no bound/oracle/cap changes or default SDK edits.

Independent fixed reviewer identified aligned boundaries (splits0) bypass
String inspection reservation before unconditional UTF16 scan aftereffect.
Add separate original-behavior control with literal 한글🙂🧪, native IDs10:0..5,
UTF16len6/UTF8len14, SV6/DSempty; start+end refusal must precede effects, then
adequate SAMEoperation must retain source/scratch bytes/SV/DS and literal.
Materialize guard remains UNFIXED in this test-first baseline; compile/runtime
NOTRUN pending separate exact SDK7 START. Old SDK20/worker43 remain historical,
original five RED preserved; no product/native/paired/fullW7 acceptance.

### Minimum aligned UTF16 validation guard after actual aligned RED

Historical38a4 SDK7 actually compiled and returned6PASS1FAIL433filtered,
SDK101/wrapper101: aligned start expectedBudget returnedOk/effect1 and no
inspected-byte refusal. Independent Unicode literal/source/scratch/SV/DS
assertions passed first; aligned end and adequate positives were NOTRUN
after that failure. Originalcommand/inputs/log/time/receipt remainimmutable.

Six-line witness-only guard now reserves the source String UTF8 byte length
BEFORE the marker/unchanged standard materialize for aligned and split paths.
That safely bounds the result substring's subsequent UTF16 validation scan.
No allocation debit is added for this scan; existing split/decode charges
remain. Removing the exact guard reconstructs ENTIRE38a4 witnesssource, and
all old28 tests/fixtures/oracles are byte-identical. No caps/defaultSDK/codec/
consumer/API/DB/UI/manifest/lock changes. CorrectedSDK7/currentold20/codec/
worker/parent/nativecost/fixedreview/realpaired/fullW7 remain NOTRUN pending
separate execution grants and acceptance; no library-only worker_done.

### Valid serialized JSON codec controls — source-only test-first baseline

Currentef7 SDK7 guards7PASS and originalSDK20 regressions20PASS separately;
unchanged rawUnicode N3 SUCCESS oracle actuallyFAIL EndOfBuffer(1), preserved
SDK101/wrapper101. Pinned Yjs13.6.32 reads exactlydeclaredN serializedJSON
Strings; exactunmodified Yrsblock.rs upstream23b7f569 readsN+1 via>=0.
OriginalrawUnicode oracle proves Rust count/involution, not Yjs JSON semantics.

Append fivevalidJSON controls only inwitness.rs: separateN0/N1/N3 exactcount
and followingfield cursor with goldenUTF8/count bytes declaredBEFOREstandard
Encoder; followingordinarynative Item10:3/mapkeyref afterlegacyJSON10:0..2,
nestedvalidJSON and exactSV4/DS0, samecapture/applyONCE/heldproof positives;
missingfield/shortString reject. LowlevelN0 is not emptygraphItem acceptance.
All old28 test/fixture bytes unchanged; removing appendedsection recovers
ENTIREef7 witness. Decoder remainsUNFIXED/original84512-byteSHA86dcf0f3...;
no oldassertErr/skip/encoder/cap/dependency/feature/parser/consumer changes.
Fivecompile/runtime NOTRUN pendingexactSTART; allold28+newfive/fixeddelta
review/currentworker/parent/nativecost/actualpaired/fullW7 remainmandatory.

### Mechanical newfixture Write trait scope repair, NOTRUN

Firste90 validJSONfive compilation failedfiveE0599 because maintainedEncoderV1
write_string is supplied by existingWrite trait. AllfiveNOTRUN/SDK101 and
secondarywrapper1 are preserved, no predicted1P4F was achieved. Sourceonly
correction adds Write import locally innew json_count_cursor andtruncated
test. Deleting exactlytwo imports recoversENTIREe90witness; allold28 and
newfive oracles/goldens/fixtures unchanged. Block.rs remainsoriginalunfixed
SHA86dcf0f3...; no type/reserve/codec/cap/defaultotherpath changes.
Separateoriginalfive START required; futureone-token fix conditionalactual
newbaseline evidence NOTsatisfied yet. ef7 precharge boundedreviewaccepted
but reviewerterminal/worktree retaineduser_takeover, not reusable/released.

### Minimum standard JSON count-loop correction after valid original RED

Actual7818 newvalid controls1PASS4FAIL440filtered/SDK101: N0/N1/N3 all
consumedfollowingZ asJSONN+1; nativefollowingMapItem10:3 decodefailed
EndOfBuffer40; propertruncatedfield/String negativesPASS. Pre-encoding
independentgolden/count/JSON/nativeID/root/key assertions reached; later
cursor/capture/apply/heldproof positives were NOTRUN afterintendedfailure.
Firstlog/time/receipt immutable; oldrawN3SUCCESSfailure preserved separately.

Only one comparison-token in ItemContent::JSON decoder changes >=0 to >0,
matchingexactN alreadywritten by maintainedencoder andpinnedYjs13.6.32.
Reversingonetoken recoversENTIRE original84512-byteupstreamblock.rs; patched
file84511bytes. Existingcounttype/reservation/encoder/defaultotherpaths/
caps/costs untouched. Entirewitness/update source andall33 test/fixture
bodies byte-identical; no assertionErr/skip/handcodec/newdependency/feature/
engine replacement. Correctedfull33/currentworker/parent/nativechildcost/
fixeddelta review/actualpair/fullW7 remainNOTRUN pendingexactnewSTART.
