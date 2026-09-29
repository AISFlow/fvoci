import type { HocuspocusProvider } from "@hocuspocus/provider";
import {
	Extension,
	type Extensions,
	type MappablePosition,
	type NodeViewRenderer,
	type Editor as TiptapEditor,
} from "@tiptap/core";
import Collaboration, { isChangeOrigin } from "@tiptap/extension-collaboration";
import CollaborationCaret from "@tiptap/extension-collaboration-caret";
import { FileHandler } from "@tiptap/extension-file-handler";
import {
	type EditorState,
	Plugin,
	PluginKey,
	TextSelection,
	type Transaction,
} from "@tiptap/pm/state";
import type { EditorView } from "@tiptap/pm/view";
import Suggestion from "@tiptap/suggestion";
import { yCursorPluginKey } from "@tiptap/y-tiptap";
import type * as Y from "yjs";
import { FVOCI_YDOC_FRAGMENT } from "./collab/constants.js";
import { type EntityResolver, isMentionEntity } from "./entities.js";
import { Attachment } from "./nodes/attachment.js";
import { Embed } from "./nodes/embed.js";
import { MathBlock, MathInline } from "./nodes/math.js";
import { Mention } from "./nodes/mention.js";
import { Mermaid } from "./nodes/mermaid.js";
import { shouldAdoptNativeOnAwareness } from "./react/awareness-selection-guard.js";
import { isNativeOwnedDeleteKey } from "./react/native-delete-owner.js";
import { parseWorkspaceUrl, resolvePastedEmbed } from "./react/paste-embed.js";
import {
	embedSlashItems,
	filterSlashItems,
	type SlashItem,
	suggestionFloatingUi,
	suggestionRenderer,
} from "./react/suggestion-menu.js";
import { createFvociExtensions, type EmojiMenuItem } from "./tiptap-schema.js";

/* The collaborative editor's extension list without a UI framework. Hosts
 * (react/fvoci-editor.tsx) supply the framework-rendered node views, the
 * loaders and the upload hooks; the schema comes only from
 * createFvociExtensions, so every host edits the same Yjs content. */

/** WHY: #749 — at textblock edges, block insertion happens outside the paragraph. */
export function uploadAnchor(
	editor: TiptapEditor,
	pos: number,
): MappablePosition {
	const $pos = editor.state.doc.resolve(pos);
	if ($pos.parent.isTextblock) {
		if ($pos.parentOffset === 0) pos = $pos.before();
		else if ($pos.parentOffset === $pos.parent.content.size) pos = $pos.after();
	}
	return editor.utils.createMappablePosition(pos);
}

/* WHY: #633 — CollaborationCaret 은 이 객체를 awareness 의 user 필드에 통째로 덮어쓴다.
 * 서버는 user.id 가 접속자와 다른 상태를 버리므로(src/collab/awareness.rs sanitize_user_state) id 는 필수다. */
export type FvociCollabUser = { id: string; name: string; color: string };

/*
 * WHY: #738 — CollaborationCaret 기본 render 는 색을 setAttribute("style", …) 로 준다.
 * 해시 기반 style-src('unsafe-inline'·'unsafe-hashes' 없음) 아래에서 style= 속성은 CSP3 §6.7.3.3
 * 상 해시로 구제되지 않아 통째로 차단되고, 피어 캐럿·라벨이 색을 잃는다. CSSOM 쓰기는 그 검사를
 * 타지 않는다 — 피어 색만 커스텀 속성으로 넘기고 규칙은 react/editor.css 의 에디터 스킨에 둔다.
 * 라벨은 캐럿의 자식이라 --afn-caret-color 를 상속한다.
 */
export function collabCaretRender(peer: {
	color: string;
	name: string;
}): HTMLElement {
	const caret = document.createElement("span");
	caret.classList.add("collaboration-carets__caret");
	caret.style.setProperty("--afn-caret-color", peer.color);
	const label = document.createElement("div");
	label.classList.add("collaboration-carets__label");
	label.textContent = peer.name;
	caret.append(label);
	return caret;
}

/* WHY: see native-delete-owner.ts. When PM's selection lags the native caret at
 * a Delete/Backspace keydown, dispatch one selection-only transaction and return
 * false so Tiptap's stock chain (undoInputRule, joins, atom handling) runs on the
 * caret the user sees. The native caret is mapped like PM's selectionFromDOM
 * (bias 1, TextSelection.between normalisation) and skipped inside
 * non-editable leaf DOM, where PM would pick a different position or a
 * NodeSelection. Awareness decoration updates are a second writer; they go
 * through createAwarenessSelectionGuardPlugin. */
function handleNativeOwnedDeleteKeyDown(
	view: EditorView,
	event: KeyboardEvent,
): boolean {
	const selection = view.state.selection;
	if (
		!isNativeOwnedDeleteKey({
			trusted: event.isTrusted,
			editable: view.editable,
			composing: event.isComposing,
			keyCode: event.keyCode,
			key: event.key,
			pmIsTextSelection: selection instanceof TextSelection,
		})
	) {
		return false;
	}
	const aligned = nativeTextSelectionFromDom(view, view.state.doc);
	if (!aligned || aligned.eq(selection)) return false;
	view.dispatch(view.state.tr.setSelection(aligned));
	return false;
}

/* WHY: awareness-only yCursorPlugin transactions rebuild caret widgets. PM then
 * calls selectionToDOM because inner decorations changed, even when
 * state.selection did not. That can clobber a native caret PM has not read yet
 * (Arrow keys, then a peer awareness message before selectionchange). Adopt
 * native only when the live DOM selection differs from PM's last-synced
 * currentSelection — native ahead, not a browser focus reset that left PM
 * ahead. Do not wrap docView.setSelection or view.dispatch: those skips also
 * drop PM's own focus writes (view.focus rAF, 20ms restore, flush doc-start
 * kludge). Do not clear suppressingSelectionUpdates (desktop Chrome never
 * sets it from selectionToDOM; clearing it would undo PM #820 on Android).
 * Mapping matches handleNativeOwnedDeleteKeyDown. Do not patch y-tiptap.
 * domObserver.currentSelection / domSelectionRange are not in
 * prosemirror-view's .d.ts; this targets the pinned @tiptap/pm view
 * (prosemirror-view 1.42.x). */
const awarenessSelectionGuardKey = new PluginKey("fvociAwarenessSelectionGuard");

type ProseMirrorDomSelectionRange = {
	anchorNode: Node | null;
	anchorOffset: number;
	focusNode: Node | null;
	focusOffset: number;
};

type ProseMirrorViewInternals = EditorView & {
	domObserver?: {
		currentSelection: { eq(other: ProseMirrorDomSelectionRange): boolean };
	};
	domSelectionRange(): ProseMirrorDomSelectionRange;
};

function nativeTextSelectionFromDom(
	view: EditorView,
	doc: EditorState["doc"],
): TextSelection | null {
	const domSel = view.dom.ownerDocument.defaultView?.getSelection();
	const anchorNode = domSel?.anchorNode;
	const focusNode = domSel?.focusNode;
	if (!domSel || !anchorNode || !focusNode) return null;
	if (!view.dom.contains(anchorNode) || !view.dom.contains(focusNode)) {
		return null;
	}
	for (const node of [anchorNode, focusNode]) {
		const element = node instanceof Element ? node : node.parentElement;
		const leaf = element?.closest('[contenteditable="false"]');
		if (leaf && leaf !== view.dom && view.dom.contains(leaf)) return null;
	}
	try {
		return TextSelection.between(
			doc.resolve(view.posAtDOM(anchorNode, domSel.anchorOffset, 1)),
			doc.resolve(view.posAtDOM(focusNode, domSel.focusOffset, 1)),
		) as TextSelection;
	} catch {
		return null;
	}
}

function observedDomSelectionMatchesNative(view: EditorView): boolean {
	const current = view as ProseMirrorViewInternals;
	const observer = current.domObserver;
	if (!observer) return true;
	return observer.currentSelection.eq(current.domSelectionRange());
}

function createAwarenessSelectionGuardPlugin(): Plugin {
	let view: EditorView | null = null;
	return new Plugin({
		key: awarenessSelectionGuardKey,
		view: (editorView) => {
			view = editorView;
			return {
				destroy() {
					view = null;
				},
			};
		},
		appendTransaction(
			transactions: readonly Transaction[],
			_old: EditorState,
			state: EditorState,
		) {
			const current = view;
			if (!current) return null;
			const awarenessUpdated = transactions.some((tr) => {
				const meta = tr.getMeta(yCursorPluginKey) as
					| { awarenessUpdated?: boolean }
					| undefined;
				return Boolean(meta?.awarenessUpdated);
			});
			if (
				!shouldAdoptNativeOnAwareness({
					awarenessUpdated,
					docChanged: transactions.some((tr) => tr.docChanged),
					selectionSet: transactions.some((tr) => tr.selectionSet),
					composing: current.composing,
					editable: current.editable,
					pmIsTextSelection: state.selection instanceof TextSelection,
					observedDomSelectionMatchesNative:
						observedDomSelectionMatchesNative(current),
				})
			) {
				return null;
			}
			const aligned = nativeTextSelectionFromDom(current, state.doc);
			if (!aligned || !aligned.empty || aligned.eq(state.selection)) return null;
			return state.tr.setSelection(aligned);
		},
	});
}

/** editorProps of the FVOCI editor: native-caret alignment before
 * Delete/Backspace and, with `ariaLabel`, the accessible name. Returns a new
 * object per call.
 *
 * WHY: #593 — 접근 가능한 이름은 ProseMirror 가 role=textbox 로 노출하는 .tiptap 에 붙어야 한다.
 * role 도 적는다 — setOptions 의 view.setProps(editorProps) 가 attributes 를 통째로 갈아끼워
 * Tiptap createView 의 role=textbox 를 지운다(EditorContent 마운트 경로). */
export function createFvociEditorProps(ariaLabel: string | undefined) {
	return {
		handleKeyDown: handleNativeOwnedDeleteKeyDown,
		...(ariaLabel
			? { attributes: { role: "textbox", "aria-label": ariaLabel } }
			: {}),
	};
}

export type MentionHit = {
	entity: string;
	id: string;
	label: string;
	title: string;
};

export type MentionLoader =
	| ((query: string) => Promise<MentionHit[]> | MentionHit[])
	| undefined;

const EMOJI_SUGGESTION_RENDER = () => suggestionRenderer<EmojiMenuItem>();

const slashKey = new PluginKey("fvociSlash");
const mentionKey = new PluginKey("fvociMention");

/* WHY: #625 — 멘션 조회가 400·네트워크로 죽어도 슬래시 메뉴의 정적 항목은 살아야 한다.
 * @tiptap/suggestion 은 items() 가 reject 하면 목록 전체를 [] 로 재발행한다. */
async function loadMentionHits(
	load: MentionLoader,
	query: string,
): Promise<MentionHit[]> {
	if (!load) return [];
	try {
		return await load(query);
	} catch {
		return [];
	}
}

function slashExtension(items: () => MentionLoader) {
	return Extension.create({
		name: "fvociSlash",
		addProseMirrorPlugins() {
			return [
				Suggestion<SlashItem, SlashItem>({
					pluginKey: slashKey,
					editor: this.editor,
					char: "/",
					// http(s) URL embed queries include `/` (`https://…`); without
					// this the match stops at `https:` and "URL 임베드" never appears.
					allowToIncludeChar: true,
					floatingUi: suggestionFloatingUi,
					/* WHY: #628 — 정적 항목은 renderer 가 쿼리에서 직접 만든다(먼저 그린다).
					 * items() 는 네트워크에 달린 임베드 후보만 돌려주고 뒤에 합쳐진다. */
					items: async ({ query, editor }) => {
						const hits = await loadMentionHits(items(), query);
						const selected = editor.state.doc.textBetween(
							editor.state.selection.from,
							editor.state.selection.to,
						);
						return embedSlashItems(query, hits, selected);
					},
					command: ({ editor, range, props }) => {
						props.run(editor, range);
					},
					render: () => suggestionRenderer<SlashItem>(filterSlashItems),
					shouldShow: ({ transaction }) => !isChangeOrigin(transaction),
				}),
			];
		},
	});
}

function mentionExtension(items: () => MentionLoader) {
	return Extension.create({
		name: "fvociMention",
		addProseMirrorPlugins() {
			return [
				Suggestion<MentionHit, MentionHit>({
					pluginKey: mentionKey,
					editor: this.editor,
					char: "@",
					floatingUi: suggestionFloatingUi,
					items: ({ query }) => loadMentionHits(items(), query),
					command: ({ editor, range, props }) => {
						editor
							.chain()
							.focus()
							.deleteRange(range)
							.insertContent([
								{
									type: "mention",
									attrs: {
										entity: props.entity,
										id: props.id,
										label: props.label,
									},
								},
								{ type: "text", text: " " },
							])
							.run();
					},
					render: () => suggestionRenderer<MentionHit>(),
					shouldShow: ({ transaction }) => !isChangeOrigin(transaction),
				}),
			];
		},
	});
}

/** Nodes whose view the host framework renders. */
export type FvociNodeViewName =
	| "mermaid"
	| "math"
	| "mathInline"
	| "embed"
	| "attachment";

/** One `addNodeView` per node, e.g. `() => ReactNodeViewRenderer(MathNodeView)`.
 * The factory applies each only as `.extend({ addNodeView })`, so a map cannot
 * change attributes or parse rules: the schema — the collab contract in
 * compat/fixtures/yjs-seed/schema.json — is the same for every host. */
export type FvociNodeViews = Readonly<
	Record<FvociNodeViewName, () => NodeViewRenderer>
>;

/** Transaction meta, set to the upload key, on the transaction that inserts a
 * finished drop/paste upload. */
export const FILE_UPLOAD_META = "fvociFileUpload";

/** Drop/paste uploads waiting for their attachment node. */
export type FvociUploadHooks = {
	/** Insertion point per upload key. The host adds one per queued file (with
	 * uploadAnchor) and deletes it when the node is inserted or the upload is
	 * removed; the extension list remaps every entry on each document change. */
	anchors: Map<string, MappablePosition>;
	/** Receives dropped files at the drop position and pasted files at the
	 * selection end. */
	queue: (editor: TiptapEditor, files: File[], pos: number) => void;
};

export type FvociEditorExtensionOptions = {
	/** The room's document, bound on FVOCI_YDOC_FRAGMENT; the host owns it. */
	ydoc: Y.Doc;
	nodeViews: FvociNodeViews;
	// Getters are read on each use, so the host can swap the values behind
	// them without rebuilding the list.
	mentionItems: () => MentionLoader;
	entityResolver: () => EntityResolver | null | undefined;
	workspaceSlug: () => string | null | undefined;
	uploads: FvociUploadHooks;
	/** Peer carets and the awareness selection guard need both. */
	provider?: HocuspocusProvider | undefined;
	user?: FvociCollabUser | undefined;
};

/** The collaborative FVOCI editor's extensions: the shared schema with the
 * host's node views, the Yjs binding, slash and `@` menus, drop/paste uploads,
 * paste-to-embed and, with a provider and user, peer carets. Every call
 * returns new extension instances. */
export function createFvociEditorExtensions(
	opts: FvociEditorExtensionOptions,
): Extensions {
	const { nodeViews, uploads } = opts;
	return [
		...createFvociExtensions({
			emojiSuggestionRender: EMOJI_SUGGESTION_RENDER,
			emojiSuggestionFloatingUi: suggestionFloatingUi,
		}).flatMap((ext) => {
			// Mention is re-added after this list with its label view, so it is
			// the last schema node (the order the editor has always had).
			if (ext.name === "mention") return [];
			if (ext.name === "mermaid") {
				return [Mermaid.extend({ addNodeView: nodeViews.mermaid })];
			}
			if (ext.name === "math") {
				return [MathBlock.extend({ addNodeView: nodeViews.math })];
			}
			if (ext.name === "mathInline") {
				return [MathInline.extend({ addNodeView: nodeViews.mathInline })];
			}
			if (ext.name === "embed") {
				return [Embed.extend({ addNodeView: nodeViews.embed })];
			}
			if (ext.name === "attachment") {
				return [Attachment.extend({ addNodeView: nodeViews.attachment })];
			}
			return [ext];
		}),
		Mention.extend({
			addNodeView() {
				return ({ node }) => {
					const dom = document.createElement("span");
					dom.setAttribute("data-mention", "");
					const entity = String(node.attrs.entity ?? "");
					const id = String(node.attrs.id ?? "");
					const stored = String(node.attrs.label ?? "");
					dom.textContent = `@${stored}`;
					const resolver = opts.entityResolver();
					if (resolver && isMentionEntity(entity)) {
						void resolver(entity, id).then((snap) => {
							if (snap) dom.textContent = `@${snap.label}`;
						});
					}
					return { dom };
				};
			},
		}),
		Collaboration.configure({
			document: opts.ydoc,
			field: FVOCI_YDOC_FRAGMENT,
		}),
		slashExtension(opts.mentionItems),
		mentionExtension(opts.mentionItems),
		FileHandler.extend({
			onTransaction({ editor: current, transaction }) {
				if (!transaction.docChanged) return;
				const completed: unknown = transaction.getMeta(FILE_UPLOAD_META);
				// WHY: #749 — Map insertion order keeps earlier files before, and later files after, this completion.
				let afterCompleted = false;
				for (const [key, anchor] of uploads.anchors) {
					if (key === completed) afterCompleted = true;
					const pos = completed
						? transaction.mapping.map(
								anchor.position,
								afterCompleted ? 1 : -1,
							)
						: current.utils.getUpdatedPosition(anchor, transaction).position
								.position;
					uploads.anchors.set(key, uploadAnchor(current, pos));
				}
			},
		}).configure({
			onDrop: (current, files, pos) => uploads.queue(current, files, pos),
			onPaste: (current, files) =>
				uploads.queue(current, files, current.state.selection.to),
		}),
		Extension.create({
			name: "fvociPasteEmbed",
			addProseMirrorPlugins() {
				const current = this.editor;
				return [
					new Plugin({
						props: {
							handlePaste(_view, event) {
								const text = event.clipboardData?.getData("text/plain") ?? "";
								const ws = opts.workspaceSlug();
								if (!ws) return false;
								const parsed = parseWorkspaceUrl(text, ws);
								if (!parsed) return false;
								const resolve = opts.entityResolver() ?? null;
								if (!resolve) {
									current
										.chain()
										.focus()
										.insertContent({
											type: "embed",
											attrs: parsed,
										})
										.run();
									return true;
								}
								const { from, to } = current.state.selection;
								void resolvePastedEmbed(text, ws, resolve).then((attrs) => {
									if (!attrs || current.isDestroyed) return;
									current
										.chain()
										.focus()
										.deleteRange({ from, to })
										.insertContentAt(from, {
											type: "embed",
											attrs,
										})
										.run();
								});
								return true;
							},
						},
					}),
				];
			},
		}),
		...(opts.provider && opts.user
			? [
					CollaborationCaret.extend({
						addProseMirrorPlugins() {
							return [
								...(this.parent?.() ?? []),
								createAwarenessSelectionGuardPlugin(),
							];
						},
					}).configure({
						provider: opts.provider,
						user: opts.user,
						render: collabCaretRender,
						/* WHY: #510 — y-tiptap 기본 selectionBuilder 는 `<color>70`(44% 알파)라
						 * 피어가 잡은 블록의 본문이 읽히지 않는다. 8% 틴트만 남긴다. */
						selectionRender: (peer: { color: string }) => ({
							class: "ProseMirror-yjs-selection",
							style: `background-color: color-mix(in srgb, ${peer.color} 8%, transparent)`,
						}),
					}),
				]
			: []),
	];
}
