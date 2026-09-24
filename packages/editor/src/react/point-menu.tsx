import * as Popover from "@radix-ui/react-popover";
import { type ReactNode, useEffect, useRef } from "react";
import { leaveMenu, useRovingMenu } from "./menu-keyboard.js";
import { overlayOwner } from "./overlay-owner.js";

export function PointMenu({
	x,
	y,
	owner,
	onClose,
	label,
	id,
	children,
}: {
	x: number;
	y: number;
	owner: HTMLElement;
	onClose: () => void;
	label: string;
	id?: string;
	children: ReactNode;
}): ReactNode {
	const ref = useRef<HTMLDivElement>(null);
	const escaped = useRef(false);
	const tabbed = useRef(false);
	const restore = useRef(owner.ownerDocument.activeElement);
	const onKeyDown = useRovingMenu(ref, true);
	useEffect(() => {
		const scroll = (event: Event) => {
			if (event.target instanceof Node && ref.current?.contains(event.target))
				return;
			onClose();
		};
		owner.ownerDocument.addEventListener("scroll", scroll, true);
		return () =>
			owner.ownerDocument.removeEventListener("scroll", scroll, true);
	}, [owner, onClose]);
	return (
		<Popover.Root
			open
			onOpenChange={(open) => {
				if (!open) onClose();
			}}
		>
			<Popover.Anchor
				virtualRef={{
					current: { getBoundingClientRect: () => new DOMRect(x, y, 0, 0) },
				}}
			/>
			<Popover.Portal container={overlayOwner(owner)}>
				<Popover.Content
					ref={ref}
					id={id}
					role="menu"
					aria-label={label}
					aria-describedby={undefined}
					className="fvoci-block-menu"
					align="start"
					collisionPadding={8}
					onKeyDown={(event) => {
						if (event.key !== "Tab") return onKeyDown(event);
						tabbed.current = true;
						leaveMenu(event, restore.current, onClose);
					}}
					onEscapeKeyDown={() => {
						escaped.current = true;
					}}
					onCloseAutoFocus={(event) => {
						event.preventDefault();
						if (tabbed.current) return;
						if (
							restore.current instanceof HTMLElement &&
							(escaped.current ||
								owner.ownerDocument.activeElement === owner.ownerDocument.body)
						)
							restore.current.focus({ preventScroll: true });
					}}
				>
					{children}
				</Popover.Content>
			</Popover.Portal>
		</Popover.Root>
	);
}
