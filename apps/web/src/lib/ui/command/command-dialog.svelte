<!-- fvoci: the default title and description are translated messages. See ../README.md. -->
<script lang="ts">
	import { m } from "$lib/i18n/paraglide/messages.js";
	import * as Dialog from "$lib/ui/dialog/index.js";
	import { cn, type WithoutChildrenOrChild } from "$lib/utils.js";
	import Command from "./command.svelte";
	import type { Command as CommandPrimitive, Dialog as DialogPrimitive } from "bits-ui";
	import type { Snippet } from "svelte";

	let {
		open = $bindable(false),
		ref = $bindable(null),
		value = $bindable(""),
		title = m.ui_command_title(),
		description = m.ui_command_description(),
		showCloseButton = false,
		portalProps,
		children,
		class: className,
		...restProps
	}: WithoutChildrenOrChild<DialogPrimitive.RootProps> &
		WithoutChildrenOrChild<CommandPrimitive.RootProps> & {
			portalProps?: DialogPrimitive.PortalProps;
			children: Snippet;
			title?: string;
			description?: string;
			showCloseButton?: boolean;
			class?: string;
		} = $props();
</script>

<Dialog.Root bind:open {...restProps}>
	<Dialog.Header class="sr-only">
		<Dialog.Title>{title}</Dialog.Title>
		<Dialog.Description>{description}</Dialog.Description>
	</Dialog.Header>
	<Dialog.Content
		class={cn("rounded-xl! top-1/3 translate-y-0 overflow-hidden p-0", className)}
		{showCloseButton}
		{portalProps}
	>
		<Command {...restProps} bind:value bind:ref {children} />
	</Dialog.Content>
</Dialog.Root>
