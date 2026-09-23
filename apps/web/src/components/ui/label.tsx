import type * as React from "react";
import { cn } from "@/lib/cn";

export function Label({ className, ...props }: React.ComponentProps<"label">) {
  return (
    <label
      className={cn("text-ui font-medium leading-none select-none", className)}
      {...props}
    />
  );
}
