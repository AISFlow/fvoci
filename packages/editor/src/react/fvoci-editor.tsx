import type { HocuspocusProvider } from "@hocuspocus/provider";
import {
	Extension,
	type MappablePosition,
	type Editor as TiptapEditor,
} from "@tiptap/core";
import Collaboration, { isChangeOrigin } from "@tiptap/extension-collaboration";
import CollaborationCaret from "@tiptap/extension-collaboration-caret";
import { FileHandler } from "@tiptap/extension-file-handler";
import {
	AllSelection,
	type EditorState,
	Plugin,
	PluginKey,
	TextSelection,
} from "@tiptap/pm/state";
import { CellSelection } from "@tiptap/pm/tables";
import type { EditorView } from "@tiptap/pm/view";
import { EditorContent, ReactNodeViewRenderer, useEditor } from "@tiptap/react";
import { BubbleMenu } from "@tiptap/react/menus";
import Suggestion from "@tiptap/suggestion";
import {
	memo,
	type ReactNode,
	useCallback,
	useContext,
	useEffect,
	useMemo,
	useRef,
	useState,
} from "react";
import * as Y from "yjs";
import { FVOCI_YDOC_FRAGMENT } from "../collab/constants.js";
import { tiptapJsonToYDoc } from "../collab-tiptap.js";
import { isTiptapDoc } from "../json.js";
import { Attachment } from "../nodes/attachment.js";
import { Embed } from "../nodes/embed.js";
import { MathBlock, MathInline } from "../nodes/math.js";
import { Mention } from "../nodes/mention.js";
import { Mermaid } from "../nodes/mermaid.js";
import { createFvociExtensions, type EmojiMenuItem } from "../tiptap-schema.js";
import {
	AttachmentBlockContext,
	AttachmentBlockView,
} from "./attachment-view.js";
import {
	type EntityResolver,
	EntityResolverContext,
	isMentionEntity,
} from "./blocks.js";
import { CodeBlockChrome } from "./code-block-chrome.js";
import { FormatToolbar } from "./format-toolbar.js";
import { Gutter } from "./gutter.js";
import { isNarrowViewport, MobileToolbar } from "./mobile-toolbar.js";
import {
	AttachmentNodeView,
	EmbedNodeView,
	MathInlineNodeView,
	MathNodeView,
	MermaidNodeView,
} from "./node-views.js";
import { isNativeOwnedDeleteKey } from "./native-delete-owner.js";
import { overlayOwner } from "./overlay-owner.js";
import { parseWorkspaceUrl, resolvePastedEmbed } from "./paste-embed.js";
import {
	embedSlashItems,
	filterSlashItems,
	type SlashItem,
	suggestionFloatingUi,
	suggestionRenderer,
} from "./suggestion-menu.js";
import { selectAllEscape, selectAllStep } from "./table-actions.js";
import { TableHandles } from "./table-handles.js";

export type { TiptapEditor };

/** WHY: #749 — at textblock edges, block insertion happens outside the paragraph. */
function uploadAnchor(editor: TiptapEditor, pos: number): MappablePosition {
	const $pos = editor.state.doc.resolve(pos);
	if ($pos.parent.isTextblock) {
		if ($pos.parentOffset === 0) pos = $pos.before();
		else if ($pos.parentOffset === $pos.parent.content.size) pos = $pos.after();
	}
	return editor.utils.createMappablePosition(pos);
}

/* WHY: #633 — CollaborationCaret 은 이 객체를 awareness 의 user 필드에 통째로 덮어쓴다.
 * 서버는 user.id 가 접속자와 다른 상태를 버리므로(apps/server/src/collab.ts) id 는 필수다. */
export type FvociCollabUser = { id: string; name: string; color: string };

/*
 * WHY: #738 — CollaborationCaret 기본 render 는 색을 setAttribute("style", …) 로 준다.
 * nonce 가 붙은 style-src 아래에서 style= 속성은 CSP3 §6.7.3.3 상 nonce 로 구제되지 않아
 * 통째로 차단되고, 피어 캐럿·라벨이 색을 잃는다. CSSOM 쓰기는 그 검사를 타지 않는다 —
 * 피어 색만 커스텀 속성으로 넘기고 규칙은 apps/web/src/index.css 의 에디터 스킨에 둔다.
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
 * NodeSelection. placeContentCaret's [1,1] case is a different writer and is not
 * handled here. */
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
	const domSel = view.dom.ownerDocument.defaultView?.getSelection();
	const anchorNode = domSel?.anchorNode;
	const focusNode = domSel?.focusNode;
	if (!domSel || !anchorNode || !focusNode) return false;
	if (!view.dom.contains(anchorNode) || !view.dom.contains(focusNode)) {
		return false;
	}
	for (const node of [anchorNode, focusNode]) {
		const element = node instanceof Element ? node : node.parentElement;
		const leaf = element?.closest('[contenteditable="false"]');
		if (leaf && leaf !== view.dom && view.dom.contains(leaf)) return false;
	}
	let aligned: TextSelection;
	try {
		const doc = view.state.doc;
		aligned = TextSelection.between(
			doc.resolve(view.posAtDOM(anchorNode, domSel.anchorOffset, 1)),
			doc.resolve(view.posAtDOM(focusNode, domSel.focusOffset, 1)),
		) as TextSelection;
	} catch {
		return false;
	}
	if (aligned.eq(selection)) return false;
	view.dispatch(view.state.tr.setSelection(aligned));
	return false;
}

export type MentionHit = {
	entity: string;
	id: string;
	label: string;
	title: string;
};

export type {
	EntityResolver,
	EntitySnapshot,
	MentionEntity,
} from "./blocks.js";

const EMOJI_SUGGESTION_RENDER = () => suggestionRenderer<EmojiMenuItem>();

/* WHY: #571 — 열린 오버레이가 Escape 를 먹는다. 에디터까지 올라가면 selectAllEscape 가 함께 돈다. */
const OVERLAY_SELECTOR =
	".fvoci-block-menu, .fvoci-ui-popover-content, .fvoci-ui-dropdown-content";

/* WHY: #571 — BubbleMenu 는 shouldShow 참조가 바뀌면 옵션 갱신 트랜잭션을 던져
 * view.updateState 를 부른다. 이 조건은 클로저를 쓰지 않으므로 모듈 상수로 둔다. */
const bubbleShouldShow = ({ state }: { state: EditorState }): boolean =>
	(state.selection instanceof TextSelection ||
		state.selection instanceof CellSelection ||
		(state.selection instanceof AllSelection &&
			state.doc.textContent.length > 0)) &&
	!state.selection.empty &&
	!isNarrowViewport();
const BUBBLE_OPTIONS = { placement: "bottom" as const };

const slashKey = new PluginKey("fvociSlash");
const mentionKey = new PluginKey("fvociMention");

function isGuardedTextField(
	target: EventTarget | null,
	host: HTMLElement | null,
): boolean {
	if (!(target instanceof HTMLElement)) return false;
	if (target.closest("input, textarea, select")) return true;
	const pm = host?.querySelector(".ProseMirror");
	if (pm?.contains(target)) return false;
	if (target.closest("[role='textbox']")) return true;
	return target.isContentEditable;
}

type MentionLoader =
	| ((query: string) => Promise<MentionHit[]> | MentionHit[])
	| undefined;

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

export const FvociEditor = memo(function FvociEditor({
	ydoc,
	initialJson,
	provider,
	user,
	editable = true,
	ariaLabel,
	mentionItems,
	entityResolver,
	workspaceSlug,
	gutterAddLabel,
	gutterMoveLabel,
	insertLabel,
	onReady,
	children,
}: {
	ydoc?: Y.Doc;
	initialJson?: unknown;
	provider?: HocuspocusProvider;
	user?: FvociCollabUser;
	editable?: boolean;
	ariaLabel?: string;
	mentionItems?: (query: string) => Promise<MentionHit[]> | MentionHit[];
	entityResolver?: EntityResolver | null;
	workspaceSlug?: string | null;
	gutterAddLabel?: string;
	gutterMoveLabel?: string;
	insertLabel?: string;
	onReady?: (editor: TiptapEditor) => void;
	children?: ReactNode;
}): ReactNode {
	const mentionRef = useRef(mentionItems);
	mentionRef.current = mentionItems;
	const entityRef = useRef(entityResolver);
	entityRef.current = entityResolver;
	const attachmentBridge = useContext(AttachmentBlockContext);
	const [uploads, setUploads] = useState<Array<{ key: string; file: File }>>(
		[],
	);
	const uploadAnchors = useRef(new Map<string, MappablePosition>());
	const uploadRef = useRef(attachmentBridge);
	uploadRef.current = attachmentBridge;
	const queueUploads = useCallback(
		(current: TiptapEditor, files: File[], pos: number) => {
			if (!uploadRef.current) return;
			const queued = files.map((file) => {
				const key = crypto.randomUUID();
				uploadAnchors.current.set(key, uploadAnchor(current, pos));
				return { key, file };
			});
			setUploads((pending) => [...pending, ...queued]);
		},
		[],
	);
	const workspaceRef = useRef(workspaceSlug);
	workspaceRef.current = workspaceSlug;
	const localDoc = useRef<Y.Doc | null>(null);
	if (localDoc.current === null) {
		if (ydoc) localDoc.current = ydoc;
		else if (isTiptapDoc(initialJson))
			localDoc.current = tiptapJsonToYDoc(initialJson);
		else localDoc.current = new Y.Doc({ gc: false });
	}
	const unreadable =
		!ydoc && initialJson !== undefined && !isTiptapDoc(initialJson);
	const doc = localDoc.current;

	/* WHY: #571 — 인라인 배열은 렌더마다 확장 30여 개를 새로 만들고, @tiptap/react 의
	 * compareOptions 가 원소 identity 로 비교해 setOptions → view.updateState 를 강제한다. */
	const extensions = useMemo(
		() => [
			...createFvociExtensions({
				emojiSuggestionRender: EMOJI_SUGGESTION_RENDER,
				emojiSuggestionFloatingUi: suggestionFloatingUi,
			}).flatMap((ext) => {
				if (ext.name === "mention") return [];
				if (ext.name === "mermaid") {
					return [
						Mermaid.extend({
							addNodeView() {
								return ReactNodeViewRenderer(MermaidNodeView);
							},
						}),
					];
				}
				if (ext.name === "math") {
					return [
						MathBlock.extend({
							addNodeView() {
								return ReactNodeViewRenderer(MathNodeView);
							},
						}),
					];
				}
				if (ext.name === "mathInline") {
					return [
						MathInline.extend({
							addNodeView() {
								return ReactNodeViewRenderer(MathInlineNodeView);
							},
						}),
					];
				}
				if (ext.name === "embed") {
					return [
						Embed.extend({
							addNodeView() {
								return ReactNodeViewRenderer(EmbedNodeView);
							},
						}),
					];
				}
				if (ext.name === "attachment") {
					return [
						Attachment.extend({
							addNodeView() {
								return ReactNodeViewRenderer(AttachmentNodeView);
							},
						}),
					];
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
						const resolver = entityRef.current;
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
				document: doc,
				field: FVOCI_YDOC_FRAGMENT,
			}),
			slashExtension(() => mentionRef.current),
			mentionExtension(() => mentionRef.current),
			FileHandler.extend({
				onTransaction({ editor: current, transaction }) {
					if (!transaction.docChanged) return;
					const completed: unknown = transaction.getMeta("fvociFileUpload");
					// WHY: #749 — Map insertion order keeps earlier files before, and later files after, this completion.
					let afterCompleted = false;
					for (const [key, anchor] of uploadAnchors.current) {
						if (key === completed) afterCompleted = true;
						const pos = completed
							? transaction.mapping.map(
									anchor.position,
									afterCompleted ? 1 : -1,
								)
							: current.utils.getUpdatedPosition(anchor, transaction).position
									.position;
						uploadAnchors.current.set(key, uploadAnchor(current, pos));
					}
				},
			}).configure({
				onDrop: (current, files, pos) => queueUploads(current, files, pos),
				onPaste: (current, files) =>
					queueUploads(current, files, current.state.selection.to),
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
									const ws = workspaceRef.current;
									if (!ws) return false;
									const parsed = parseWorkspaceUrl(text, ws);
									if (!parsed) return false;
									const resolve = entityRef.current ?? null;
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
			...(provider && user
				? [
						CollaborationCaret.configure({
							provider,
							user,
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
		],
		[doc, provider, user, queueUploads],
	);

	/* WHY: #593 — 접근 가능한 이름은 ProseMirror 가 role=textbox 로 노출하는 .tiptap 에 붙어야 한다.
	 * #571 — compareOptions 는 editorProps 를 identity 로 비교하므로 참조를 고정한다.
	 * role 도 적는다 — setOptions 의 view.setProps(editorProps) 가 attributes 를 통째로 갈아끼워
	 * Tiptap createView 의 role=textbox 를 지운다(EditorContent 마운트 경로). */
	const editorProps = useMemo(
		() => ({
			handleKeyDown: handleNativeOwnedDeleteKeyDown,
			...(ariaLabel
				? { attributes: { role: "textbox", "aria-label": ariaLabel } }
				: {}),
		}),
		[ariaLabel],
	);

	/* WHY: #738 — Tiptap 기본값은 nonce 없는 <style data-tiptap-style> 을 head 에 꽂는다
	 * (@tiptap/core createStyleTag). nonce 가 붙은 style-src 아래에서 'unsafe-inline' 은
	 * 죽어 있으므로 그 <style> 은 통째로 차단된다 — 규칙은 apps/web/src/index.css 로 옮겼다. */
	const editor = useEditor({
		immediatelyRender: false,
		injectCSS: false,
		editable,
		extensions,
		editorProps,
	});

	const bubbleAppendTo = useMemo(
		() => (editor ? () => overlayOwner(editor.view.dom) : undefined),
		[editor],
	);

	const readyRef = useRef(onReady);
	readyRef.current = onReady;
	const hostRef = useRef<HTMLDivElement>(null);
	useEffect(() => {
		if (editor) readyRef.current?.(editor);
	}, [editor]);

	useEffect(() => {
		if (editor) editor.setEditable(editable);
	}, [editor, editable]);

	// WHY: 문서·태스크 본문은 페이지당 FvociEditor 하나 — 여러 개면 뷰포트에 보이는 첫 host 로 바꿔야 한다.
	useEffect(() => {
		if (!editor) return;
		const host = hostRef.current;
		const onKeyDown = (event: KeyboardEvent) => {
			if (!host?.isConnected) return;
			if (isGuardedTextField(event.target, host)) return;
			if (event.key === "Escape") {
				if (document.querySelector(OVERLAY_SELECTOR)) return;
				if (selectAllEscape(editor)) event.preventDefault();
				return;
			}
			if (event.key !== "a" && event.key !== "A") return;
			if (!(event.metaKey || event.ctrlKey) || event.altKey) return;
			event.preventDefault();
			selectAllStep(editor);
		};
		const onMouseDown = (event: MouseEvent) => {
			if (event.target !== host) return;
			event.preventDefault();
			editor.commands.focus("end");
		};
		document.addEventListener("keydown", onKeyDown, true);
		host?.addEventListener("mousedown", onMouseDown);
		return () => {
			document.removeEventListener("keydown", onKeyDown, true);
			host?.removeEventListener("mousedown", onMouseDown);
		};
	}, [editor]);

	if (unreadable) return null;
	if (!editor) return null;

	return (
		<EntityResolverContext.Provider value={entityResolver ?? null}>
			<div ref={hostRef} className="fvoci-editor relative min-h-[16rem]">
				<BubbleMenu
					editor={editor}
					updateDelay={0}
					options={BUBBLE_OPTIONS}
					appendTo={bubbleAppendTo}
					data-fvoci-bubble=""
					className="fvoci-bubble"
					shouldShow={bubbleShouldShow}
				>
					<FormatToolbar editor={editor} />
				</BubbleMenu>
				<EditorContent editor={editor} />
				{uploads.length > 0 ? (
					<div
						data-fvoci-uploads=""
						className="sticky bottom-0 z-10 flex flex-col gap-1 bg-background py-1"
					>
						{uploads.map(({ key, file }) => (
							<AttachmentBlockView
								key={key}
								initialFile={file}
								blockProps={{
									id: "",
									name: file.name,
									image: false,
									width: 100,
									align: "center",
									caption: "",
									previewWidth: 0,
									previewHeight: 0,
								}}
								readOnly={!editable}
								onRemove={() => {
									uploadAnchors.current.delete(key);
									setUploads((pending) =>
										pending.filter((item) => item.key !== key),
									);
								}}
								onUploaded={(result) => {
									const anchor = uploadAnchors.current.get(key);
									if (editor.isDestroyed || !anchor) return;
									editor
										.chain()
										.setMeta("fvociFileUpload", key)
										.insertContentAt(
											anchor.position,
											{ type: "attachment", attrs: result },
											{ updateSelection: false },
										)
										.run();
									uploadAnchors.current.delete(key);
									setUploads((pending) =>
										pending.filter((item) => item.key !== key),
									);
								}}
							/>
						))}
					</div>
				) : null}
				{editable ? (
					<Gutter
						editor={editor}
						addLabel={gutterAddLabel}
						moveLabel={gutterMoveLabel}
					/>
				) : null}
				{editable ? <TableHandles editor={editor} /> : null}
				{editable ? (
					<MobileToolbar editor={editor} insertLabel={insertLabel} />
				) : null}
				<CodeBlockChrome editor={editor} />
				{children}
			</div>
		</EntityResolverContext.Provider>
	);
});
