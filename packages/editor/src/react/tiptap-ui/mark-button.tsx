import { t } from "@fvoci/i18n";
import type { Editor } from "@tiptap/react";
import { useEditorState } from "@tiptap/react";
import type { ReactNode } from "react";
import { Button } from "../tiptap-ui-primitive/button.js";

export type MarkType = "bold" | "italic" | "underline" | "strike" | "code";

const LABEL: Record<MarkType, string> = {
	bold: "B",
	italic: "I",
	underline: "U",
	strike: "S",
	code: "</>",
};

export function MarkButton({
	editor,
	type,
}: {
	editor: Editor;
	type: MarkType;
}): ReactNode {
	const pressed = useEditorState({
		editor,
		selector: ({ editor: current }) => current.isActive(type),
	});
	return (
		<Button
			aria-pressed={pressed}
			aria-label={t(`editor.mark.${type}`)}
			onClick={() => {
				editor.chain().focus().toggleMark(type).run();
			}}
		>
			{LABEL[type]}
		</Button>
	);
}
