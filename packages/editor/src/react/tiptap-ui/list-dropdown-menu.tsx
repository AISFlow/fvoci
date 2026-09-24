import { t } from "@fvoci/i18n";
import type { Editor } from "@tiptap/react";
import type { ReactNode } from "react";
import { Button } from "../tiptap-ui-primitive/button.js";
import {
	DropdownMenu,
	DropdownMenuContent,
} from "../tiptap-ui-primitive/dropdown-menu.js";
import { ListButton } from "./list-button.js";

export function ListDropdownMenu({ editor }: { editor: Editor }): ReactNode {
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
						{t("editor.list")}
					</Button>
					<DropdownMenuContent open={open} id={id} label={t("editor.list")}>
						<ListButton
							editor={editor}
							type="bulletList"
							label={t("editor.block.bullet")}
						/>
						<ListButton
							editor={editor}
							type="orderedList"
							label={t("editor.block.ordered")}
						/>
						<ListButton
							editor={editor}
							type="taskList"
							label={t("editor.block.task")}
						/>
					</DropdownMenuContent>
				</>
			)}
		</DropdownMenu>
	);
}
