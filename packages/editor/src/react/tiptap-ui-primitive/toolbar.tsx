import type { ReactNode } from "react";

export function Toolbar({
	children,
	className,
}: {
	children: ReactNode;
	className?: string;
}): ReactNode {
	return (
		<div
			className={
				className ? `fvoci-ui-toolbar ${className}` : "fvoci-ui-toolbar"
			}
			role="toolbar"
		>
			{children}
		</div>
	);
}
