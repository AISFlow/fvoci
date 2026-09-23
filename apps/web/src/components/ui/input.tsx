import type * as React from "react";
import { cn } from "@/lib/cn";

export function Input({ className, ...props }: React.ComponentProps<"input">) {
  return (
    <input
      className={cn(
        "h-11 w-full min-w-0 rounded-md border border-input bg-background px-3 py-2 text-ui outline-none focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring disabled:opacity-50",
        className,
      )}
      {...props}
    />
  );
}
