import { t } from "@fvoci/i18n";
import type { Editor } from "@tiptap/react";
import type { ReactNode } from "react";
import { ColorHighlightPopover } from "./tiptap-ui/color-highlight-popover.js";
import { HeadingDropdownMenu } from "./tiptap-ui/heading-dropdown-menu.js";
import { LinkPopover } from "./tiptap-ui/link-popover.js";
import { ListDropdownMenu } from "./tiptap-ui/list-dropdown-menu.js";
import { MarkButton } from "./tiptap-ui/mark-button.js";
import { TextAlignButton } from "./tiptap-ui/text-align-button.js";
import { Button } from "./tiptap-ui-primitive/button.js";
import {
	DropdownMenu,
	DropdownMenuContent,
} from "./tiptap-ui-primitive/dropdown-menu.js";
import { Separator } from "./tiptap-ui-primitive/separator.js";
import { Toolbar } from "./tiptap-ui-primitive/toolbar.js";

const MARKS = ["bold", "italic", "underline", "strike", "code"] as const;

/** Selection and mobile formatting share one grouped control set. */
export function FormatToolbar({ editor }: { editor: Editor }): ReactNode {
	return (
		<Toolbar>
			<div className="fvoci-format-cluster">
				<HeadingDropdownMenu editor={editor} />
				<ListDropdownMenu editor={editor} />
			</div>
			<div className="fvoci-format-cluster">
				{MARKS.map((type) => (
					<MarkButton key={type} editor={editor} type={type} />
				))}
			</div>
			<LinkPopover editor={editor} />
			<ColorHighlightPopover editor={editor} />
			<DropdownMenu>
				{({ open, setOpen, id }) => (
					<>
						<Button
							aria-label={t("editor.format")}
							aria-haspopup="menu"
							aria-expanded={open}
							aria-controls={id}
							onClick={() => setOpen(!open)}
						>
							⋮
						</Button>
						<DropdownMenuContent open={open} id={id} label={t("editor.format")}>
							<TextAlignButton
								editor={editor}
								align="left"
								label={t("editor.align.left")}
							/>
							<TextAlignButton
								editor={editor}
								align="center"
								label={t("editor.align.center")}
							/>
							<TextAlignButton
								editor={editor}
								align="right"
								label={t("editor.align.right")}
							/>
							<Separator />
							<Button
								role="menuitem"
								onClick={() => {
									editor.chain().focus().unsetAllMarks().run();
									setOpen(false);
								}}
							>
								{t("editor.format.clear")}
							</Button>
						</DropdownMenuContent>
					</>
				)}
			</DropdownMenu>
		</Toolbar>
	);
}
