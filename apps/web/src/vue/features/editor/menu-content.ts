import type { PopoverProps } from "@nuxt/ui/components/Popover.vue";

type PopoverContent = NonNullable<PopoverProps["content"]>;

/**
 * UPopover content options for the editor's menus and popovers (the React
 * primitives' placement: start-aligned, 8px from the viewport edges).
 * asChild makes the slot's own element the popover content, so its role
 * (a menu is role="menu", not Reka's "dialog"), id and label are the ones
 * the trigger's aria-controls names. UPopover's `content` type leaves
 * asChild out but forwards it to Reka's PopoverContent.
 */
export function menuContent(
  options: Pick<
    PopoverContent,
    "side" | "sideOffset" | "onOpenAutoFocus" | "onEscapeKeyDown" | "onCloseAutoFocus" | "onFocusOutside"
  >,
): PopoverContent {
  return { align: "start", collisionPadding: 8, ...options, asChild: true } as PopoverContent;
}
