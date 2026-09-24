import { t } from "@fvoci/i18n";
import type { Editor } from "@tiptap/react";
import type { ReactNode } from "react";
import { FormatToolbar } from "./format-toolbar.js";
import { insertSlashHere } from "./gutter-actions.js";
import { Button } from "./tiptap-ui-primitive/button.js";

export function isNarrowViewport(): boolean {
	return window.matchMedia("(max-width: 47.999rem)").matches;
}

export function MobileToolbar({
	editor,
	insertLabel = t("editor.mobile.insert"),
}: {
	editor: Editor;
	insertLabel?: string;
}): ReactNode {
	return (
		<div className="fvoci-mobile-toolbar" data-mobile-toolbar="">
			<FormatToolbar editor={editor} />
			<Button
				aria-label={insertLabel}
				onClick={() => {
					insertSlashHere(editor);
				}}
			>
				+
			</Button>
		</div>
	);
}
