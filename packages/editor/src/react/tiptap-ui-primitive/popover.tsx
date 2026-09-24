import * as Primitive from "@radix-ui/react-popover";
import {
	createContext,
	type ReactNode,
	type Ref,
	useContext,
	useId,
	useRef,
	useState,
} from "react";
import { leaveMenu } from "../menu-keyboard.js";
import { overlayOwner } from "../overlay-owner.js";

const Host = createContext<{
	element: HTMLElement | null;
	setOpen: (open: boolean) => void;
} | null>(null);

export function Popover({
	children,
}: {
	children: (opts: {
		open: boolean;
		setOpen: (open: boolean) => void;
		id: string;
	}) => ReactNode;
}): ReactNode {
	const [open, setOpen] = useState(false);
	const [host, setHost] = useState<HTMLDivElement | null>(null);
	const id = useId();
	return (
		<Primitive.Root open={open} onOpenChange={setOpen}>
			<Host.Provider value={{ element: host, setOpen }}>
				<Primitive.Anchor asChild>
					<div
						ref={setHost}
						className="fvoci-ui-popover"
						data-open={String(open)}
					>
						{children({ open, setOpen, id })}
					</div>
				</Primitive.Anchor>
			</Host.Provider>
		</Primitive.Root>
	);
}

export function PopoverContent({
	open,
	id,
	label,
	children,
	...props
}: {
	open: boolean;
	id: string;
	label: string;
	children: ReactNode;
} & Pick<Primitive.PopoverContentProps, "role" | "className" | "onKeyDown"> & {
		ref?: Ref<HTMLDivElement>;
	}): ReactNode {
	const context = useContext(Host);
	const host = context?.element;
	const escaped = useRef(false);
	const tabbed = useRef(false);
	if (!open || !host) return null;
	return (
		<Primitive.Portal container={overlayOwner(host)}>
			<Primitive.Content
				id={id}
				aria-label={label}
				aria-describedby={undefined}
				className="fvoci-ui-popover-content"
				side={host.closest("[data-mobile-toolbar]") ? "top" : "bottom"}
				align="start"
				sideOffset={4}
				collisionPadding={8}
				onEscapeKeyDown={() => {
					escaped.current = true;
				}}
				onCloseAutoFocus={(event) => {
					event.preventDefault();
					if (tabbed.current) {
						tabbed.current = false;
						return;
					}
					if (
						escaped.current ||
						host.ownerDocument.activeElement === host.ownerDocument.body
					) {
						escaped.current = false;
						host
							.querySelector<HTMLElement>("button")
							?.focus({ preventScroll: true });
					}
				}}
				onInteractOutside={(event) => {
					if (event.target instanceof Node && host.contains(event.target))
						event.preventDefault();
				}}
				{...props}
				onKeyDown={(event) => {
					if (props.role !== "menu" || event.key !== "Tab") {
						props.onKeyDown?.(event);
						return;
					}
					tabbed.current = true;
					leaveMenu(event, host.querySelector("button"), () =>
						context?.setOpen(false),
					);
				}}
			>
				{children}
			</Primitive.Content>
		</Primitive.Portal>
	);
}
