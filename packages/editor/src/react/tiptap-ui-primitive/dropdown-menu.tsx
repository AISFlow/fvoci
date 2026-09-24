import { type ReactNode, useRef } from "react";
import { useRovingMenu } from "../menu-keyboard.js";
import { Popover, PopoverContent } from "./popover.js";

export const DropdownMenu = Popover;

export function DropdownMenuContent({
	open,
	id,
	label,
	children,
}: {
	open: boolean;
	id: string;
	label: string;
	children: ReactNode;
}): ReactNode {
	const ref = useRef<HTMLDivElement>(null);
	const onKeyDown = useRovingMenu(ref, open);
	return (
		<PopoverContent
			open={open}
			id={id}
			label={label}
			ref={ref}
			className="fvoci-ui-dropdown-content"
			role="menu"
			onKeyDown={onKeyDown}
		>
			{children}
		</PopoverContent>
	);
}
