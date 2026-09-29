import type { HocuspocusProvider } from "@hocuspocus/provider";
import type { MappablePosition, Editor as TiptapEditor } from "@tiptap/core";
import {
	AllSelection,
	type EditorState,
	TextSelection,
} from "@tiptap/pm/state";
import { CellSelection } from "@tiptap/pm/tables";
import { EditorContent, ReactNodeViewRenderer, useEditor } from "@tiptap/react";
import { BubbleMenu } from "@tiptap/react/menus";
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
import { tiptapJsonToYDoc } from "../collab-tiptap.js";
import {
	createFvociEditorExtensions,
	createFvociEditorProps,
	FILE_UPLOAD_META,
	type FvociCollabUser,
	type FvociNodeViews,
	type MentionHit,
	uploadAnchor,
} from "../editor-extensions.js";
import type { EntityResolver } from "../entities.js";
import { isTiptapDoc } from "../json.js";
import {
	AttachmentBlockContext,
	AttachmentBlockView,
} from "./attachment-view.js";
import { EntityResolverContext } from "./blocks.js";
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
import { overlayOwner } from "./overlay-owner.js";
import { selectAllEscape, selectAllStep } from "./table-actions.js";
import { TableHandles } from "./table-handles.js";

export type { TiptapEditor };

export {
	collabCaretRender,
	type FvociCollabUser,
	type MentionHit,
} from "../editor-extensions.js";

export type {
	EntityResolver,
	EntitySnapshot,
	MentionEntity,
} from "../entities.js";

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

const REACT_NODE_VIEWS: FvociNodeViews = {
	mermaid: () => ReactNodeViewRenderer(MermaidNodeView),
	math: () => ReactNodeViewRenderer(MathNodeView),
	mathInline: () => ReactNodeViewRenderer(MathInlineNodeView),
	embed: () => ReactNodeViewRenderer(EmbedNodeView),
	attachment: () => ReactNodeViewRenderer(AttachmentNodeView),
};

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
		() =>
			createFvociEditorExtensions({
				ydoc: doc,
				nodeViews: REACT_NODE_VIEWS,
				mentionItems: () => mentionRef.current,
				entityResolver: () => entityRef.current,
				workspaceSlug: () => workspaceRef.current,
				uploads: { anchors: uploadAnchors.current, queue: queueUploads },
				provider,
				user,
			}),
		[doc, provider, user, queueUploads],
	);

	/* WHY: #571 — compareOptions 는 editorProps 를 identity 로 비교하므로 참조를 고정한다. */
	const editorProps = useMemo(
		() => createFvociEditorProps(ariaLabel),
		[ariaLabel],
	);

	/* WHY: #738 — Tiptap 기본값은 <style data-tiptap-style> 을 head 에 꽂는다
	 * (@tiptap/core createStyleTag). style-src 는 'self' 와 셸 인라인 블록의 빌드 시점 해시뿐이고
	 * 'unsafe-inline' 이 없으므로 그 <style> 은 통째로 차단된다 — 규칙은 react/editor.css 로 옮겼다. */
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
										.setMeta(FILE_UPLOAD_META, key)
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
