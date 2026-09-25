import type * as React from "react";
import { cn } from "@/lib/cn";

type ButtonProps = React.ComponentProps<"button"> & {
  variant?: "default" | "outline" | "link" | "destructive";
  size?: "default" | "sm" | "lg";
};

export function Button({
  className,
  variant = "default",
  size = "default",
  ...props
}: ButtonProps) {
  return (
    <button
      className={cn(
        "inline-flex items-center justify-center rounded-md border border-transparent text-ui font-medium transition-colors disabled:pointer-events-none disabled:opacity-50",
        variant === "default" && "bg-primary text-primary-foreground hover:opacity-90",
        variant === "outline" &&
          "border-border bg-background hover:bg-accent hover:text-foreground",
        variant === "link" && "h-auto min-h-11 justify-start px-0 text-primary underline-offset-4 hover:underline",
        variant === "destructive" && "bg-destructive text-white hover:opacity-90",
        size === "default" && "h-10 px-4",
        size === "sm" && "h-8 px-3 text-sm",
        size === "lg" && "h-11 px-6",
        className,
      )}
      {...props}
    />
  );
}
