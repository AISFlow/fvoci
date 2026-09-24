import { t } from "@fvoci/i18n";
import type { Editor } from "@tiptap/react";
import { useEditorState } from "@tiptap/react";
import { type FormEvent, type ReactNode, useState } from "react";
import { Button } from "../tiptap-ui-primitive/button.js";
import { Input } from "../tiptap-ui-primitive/input.js";
import { Popover, PopoverContent } from "../tiptap-ui-primitive/popover.js";

export function LinkPopover({ editor }: { editor: Editor }): ReactNode {
	const [href, setHref] = useState("");
	const active = useEditorState({
		editor,
		selector: ({ editor: current }) => current.isActive("link"),
	});
	const apply = (event?: FormEvent) => {
		event?.preventDefault();
		const next = href.trim();
		if (next.length === 0) return;
		editor.chain().focus().setLink({ href: next }).run();
	};
	return (
		<Popover>
			{({ open, setOpen, id }) => (
				<>
					<Button
						aria-pressed={active}
						aria-haspopup="dialog"
						aria-expanded={open}
						aria-controls={id}
						onClick={() => {
							const current = editor.getAttributes("link").href;
							setHref(typeof current === "string" ? current : "");
							setOpen(!open);
						}}
					>
						{t("editor.link")}
					</Button>
					<PopoverContent open={open} id={id} label={t("editor.link")}>
						<form onSubmit={apply}>
							<Input
								type="text"
								aria-label="URL"
								value={href}
								onChange={(event) => setHref(event.target.value)}
								placeholder="https://"
							/>
							<Button type="submit">{t("editor.link.apply")}</Button>
						</form>
					</PopoverContent>
				</>
			)}
		</Popover>
	);
}
