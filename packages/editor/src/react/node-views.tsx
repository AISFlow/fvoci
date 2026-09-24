import {
	type NodeViewProps,
	NodeViewWrapper,
	useEditorState,
} from "@tiptap/react";
import type { ReactNode } from "react";
import { AttachmentBlockView } from "./attachment-view.js";
import {
	EmbedBlockView,
	isEmbedEntity,
	MathBlockView,
	MathInlineView,
	MermaidBlockView,
} from "./blocks.js";
import { useMathMl } from "./katex.js";
import { ATTACHMENT_ALIGNS, ATTACHMENT_WIDTH } from "./preview-registry.js";

/* WHY: #644 F6 — setEditable 은 노드 객체를 그대로 두고 ReactNodeView.update 가
 * `node === this.node` 에서 단락하므로 editor.isEditable 을 그냥 읽으면 토글이 반영되지 않는다.
 * useEditorState 는 transaction·update 를 모두 구독하고 setEditable 은 update 를 emit 한다. */
function useIsEditable(editor: NodeViewProps["editor"]): boolean {
	return useEditorState({
		editor,
		selector: (snapshot) => snapshot.editor.isEditable,
	});
}

export function MathNodeView({
	node,
	editor,
	updateAttributes,
}: NodeViewProps): ReactNode {
	const editable = useIsEditable(editor);
	const latex = typeof node.attrs.latex === "string" ? node.attrs.latex : "";
	const { html, failed } = useMathMl(latex);
	return (
		<NodeViewWrapper>
			<MathBlockView
				latex={latex}
				editable={editable}
				html={html}
				failed={failed}
				onCommit={(next) => {
					updateAttributes({ latex: next });
				}}
			/>
		</NodeViewWrapper>
	);
}

export function MathInlineNodeView({
	node,
	editor,
	updateAttributes,
}: NodeViewProps): ReactNode {
	const editable = useIsEditable(editor);
	const latex = typeof node.attrs.latex === "string" ? node.attrs.latex : "";
	const { html, failed } = useMathMl(latex, false);
	return (
		<NodeViewWrapper as="span">
			<MathInlineView
				latex={latex}
				editable={editable}
				html={html}
				failed={failed}
				onCommit={(next) => {
					updateAttributes({ latex: next });
				}}
			/>
		</NodeViewWrapper>
	);
}

export function MermaidNodeView({
	node,
	editor,
	updateAttributes,
}: NodeViewProps): ReactNode {
	const editable = useIsEditable(editor);
	const source = typeof node.attrs.source === "string" ? node.attrs.source : "";
	return (
		<NodeViewWrapper>
			<MermaidBlockView
				code={source}
				editable={editable}
				svg={null}
				failed={false}
				onCommit={(code) => {
					updateAttributes({ source: code });
				}}
			/>
		</NodeViewWrapper>
	);
}

export function EmbedNodeView({
	node,
	editor,
	updateAttributes,
}: NodeViewProps): ReactNode {
	const editable = useIsEditable(editor);
	const entityRaw = String(node.attrs.entity ?? "document");
	const entity = isEmbedEntity(entityRaw) ? entityRaw : "document";
	const refValue = String(node.attrs.ref ?? "");
	return (
		<NodeViewWrapper>
			<EmbedBlockView
				entity={entity}
				refValue={refValue}
				editable={editable}
				onCommit={(next) => {
					updateAttributes(next);
				}}
			/>
		</NodeViewWrapper>
	);
}

export function AttachmentNodeView({
	node,
	editor,
	getPos,
	updateAttributes,
	deleteNode,
}: NodeViewProps): ReactNode {
	const editable = useIsEditable(editor);
	const attrs = node.attrs;
	return (
		<NodeViewWrapper>
			<AttachmentBlockView
				blockProps={{
					id: typeof attrs.id === "string" ? attrs.id : "",
					name: typeof attrs.name === "string" ? attrs.name : "",
					image: attrs.image === true,
					width:
						typeof attrs.width === "number"
							? attrs.width
							: ATTACHMENT_WIDTH.max,
					align: ATTACHMENT_ALIGNS.find((a) => a === attrs.align) ?? "center",
					caption: typeof attrs.caption === "string" ? attrs.caption : "",
					previewWidth:
						typeof attrs.previewWidth === "number" ? attrs.previewWidth : 0,
					previewHeight:
						typeof attrs.previewHeight === "number" ? attrs.previewHeight : 0,
				}}
				readOnly={!editable}
				onUploaded={(result) => {
					/* WHY: #644 F8 — 업로드 완료 되쓰기는 사용자의 스텝이 아니다. y-tiptap 의
					 * UndoManager 는 captureTransaction 에서 이 meta 를 보므로 Mod-z 가
					 * 삽입 자체를 되돌리고, placeholder 로 되돌아가는 중간 스텝은 남지 않는다. */
					const pos = getPos();
					if (pos === undefined) return;
					const { view } = editor;
					view.dispatch(
						view.state.tr
							.setNodeAttribute(pos, "id", result.id)
							.setNodeAttribute(pos, "name", result.name)
							.setNodeAttribute(pos, "image", result.image)
							.setMeta("addToHistory", false),
					);
				}}
				onRemove={deleteNode}
				onProps={updateAttributes}
			/>
		</NodeViewWrapper>
	);
}
