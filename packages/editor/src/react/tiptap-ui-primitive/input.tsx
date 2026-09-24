import { forwardRef, type InputHTMLAttributes } from "react";

export const Input = forwardRef<
	HTMLInputElement,
	InputHTMLAttributes<HTMLInputElement>
>(function Input({ className, ...props }, ref) {
	const cls = className ? `fvoci-ui-input ${className}` : "fvoci-ui-input";
	return <input ref={ref} className={cls} {...props} />;
});
