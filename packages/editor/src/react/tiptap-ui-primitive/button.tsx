import { type ButtonHTMLAttributes, forwardRef, type ReactNode } from "react";

export type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
	tooltip?: ReactNode;
};

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(
	function Button({ className, children, type = "button", ...props }, ref) {
		const cls = className ? `fvoci-ui-button ${className}` : "fvoci-ui-button";
		return (
			<button ref={ref} type={type} className={cls} {...props}>
				{children}
			</button>
		);
	},
);
