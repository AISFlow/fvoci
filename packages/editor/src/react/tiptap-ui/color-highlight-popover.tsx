import { type I18nKey, t } from "@fvoci/i18n";
import type { Editor } from "@tiptap/react";
import { useEditorState } from "@tiptap/react";
import type { ReactNode } from "react";
import { Button } from "../tiptap-ui-primitive/button.js";
import { Popover, PopoverContent } from "../tiptap-ui-primitive/popover.js";

const COLORS: Array<{ key: I18nKey; value: string }> = [
	{ key: "editor.color.yellow", value: "var(--accent)" },
	{ key: "editor.color.red", value: "var(--destructive)" },
	{ key: "editor.color.muted", value: "var(--muted)" },
];

export function ColorHighlightPopover({
	editor,
}: {
	editor: Editor;
}): ReactNode {
	const active = useEditorState({
		editor,
		selector: ({ editor: current }) => current.isActive("highlight"),
	});
	return (
		<Popover>
			{({ open, setOpen, id }) => (
				<>
					<Button
						aria-pressed={active}
						aria-haspopup="dialog"
						aria-expanded={open}
						aria-controls={id}
						onClick={() => setOpen(!open)}
					>
						{t("editor.color")}
					</Button>
					<PopoverContent
						open={open}
						id={id}
						label={t("editor.color.highlight")}
					>
						{COLORS.map((color) => (
							<Button
								key={color.key}
								aria-label={t(color.key)}
								onClick={() => {
									editor
										.chain()
										.focus()
										.toggleHighlight({ color: color.value })
										.run();
									setOpen(false);
								}}
							>
								{t(color.key)}
							</Button>
						))}
					</PopoverContent>
				</>
			)}
		</Popover>
	);
}
