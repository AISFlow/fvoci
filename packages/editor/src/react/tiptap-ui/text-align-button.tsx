import type { Editor } from "@tiptap/react";
import { useEditorState } from "@tiptap/react";
import type { ReactNode } from "react";
import { Button } from "../tiptap-ui-primitive/button.js";

export function TextAlignButton({
	editor,
	align,
	label,
}: {
	editor: Editor;
	align: "left" | "center" | "right";
	label: string;
}): ReactNode {
	const pressed = useEditorState({
		editor,
		selector: ({ editor: current }) => current.isActive({ textAlign: align }),
	});
	return (
		<Button
			role="menuitemradio"
			aria-checked={pressed}
			onClick={() => {
				editor.chain().focus().setTextAlign(align).run();
			}}
		>
			{label}
		</Button>
	);
}
