import { t } from "@fvoci/i18n";
import type { Editor } from "@tiptap/react";
import { useEditorState } from "@tiptap/react";
import type { ReactNode } from "react";
import { Button } from "../tiptap-ui-primitive/button.js";
import {
	DropdownMenu,
	DropdownMenuContent,
} from "../tiptap-ui-primitive/dropdown-menu.js";

const LEVELS = [1, 2, 3] as const;

function headingTrigger(editor: Editor): string {
	for (const level of LEVELS) {
		if (editor.isActive("heading", { level })) return `H${level}▾`;
	}
	return `${t("editor.block.paragraph")}▾`;
}

export function HeadingDropdownMenu({ editor }: { editor: Editor }): ReactNode {
	const trigger = useEditorState({
		editor,
		selector: ({ editor: current }) => headingTrigger(current),
	});
	return (
		<DropdownMenu>
			{({ open, setOpen, id }) => (
				<>
					<Button
						aria-haspopup="menu"
						aria-expanded={open}
						aria-controls={id}
						onClick={() => setOpen(!open)}
					>
						{trigger}
					</Button>
					<DropdownMenuContent
						open={open}
						id={id}
						label={t("editor.block.type")}
					>
						<Button
							role="menuitemradio"
							aria-checked={
								!LEVELS.some((level) => editor.isActive("heading", { level }))
							}
							onClick={() => {
								editor.chain().focus().setParagraph().run();
								setOpen(false);
							}}
						>
							{t("editor.block.paragraph")}
						</Button>
						{LEVELS.map((level) => (
							<Button
								key={level}
								role="menuitemradio"
								aria-checked={editor.isActive("heading", { level })}
								onClick={() => {
									editor.chain().focus().setHeading({ level }).run();
									setOpen(false);
								}}
							>
								{`H${level}`}
							</Button>
						))}
					</DropdownMenuContent>
				</>
			)}
		</DropdownMenu>
	);
}
