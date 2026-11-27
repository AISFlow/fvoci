<!-- fvoci: room for the close button and normal line height, for titles of two lines. See ../README.md. -->
<script lang="ts">
	import { Dialog as DialogPrimitive } from "bits-ui";
	import { cn } from "$lib/utils.js";

	let {
		ref = $bindable(null),
		class: className,
		...restProps
	}: DialogPrimitive.TitleProps = $props();
</script>

<DialogPrimitive.Title
	bind:ref
	data-slot="dialog-title"
	class={cn("pr-8 text-base font-medium", className)}
	{...restProps}
/>
