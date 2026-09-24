import type { Editor } from "@tiptap/react";
import type { ReactNode } from "react";
import { Button } from "../tiptap-ui-primitive/button.js";

export function ListButton({
	editor,
	type,
	label,
}: {
	editor: Editor;
	type: "bulletList" | "orderedList" | "taskList";
	label: string;
}): ReactNode {
	return (
		<Button
			role="menuitemcheckbox"
			aria-checked={editor.isActive(type)}
			onClick={() => {
				if (type === "bulletList")
					editor.chain().focus().toggleBulletList().run();
				else if (type === "orderedList")
					editor.chain().focus().toggleOrderedList().run();
				else editor.chain().focus().toggleTaskList().run();
			}}
		>
			{label}
		</Button>
	);
}
