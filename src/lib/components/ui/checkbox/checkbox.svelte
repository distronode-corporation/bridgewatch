<script lang="ts">
	import { Checkbox as CheckboxPrimitive } from "bits-ui";
	import CheckIcon from '@lucide/svelte/icons/check';
	import { cn, type WithoutChildrenOrChild } from "$lib/components/ui/utils.js";

	let {
		ref = $bindable(null),
		checked = $bindable(false),
		class: className,
		...restProps
	}: Omit<WithoutChildrenOrChild<CheckboxPrimitive.RootProps>, "indeterminate"> = $props();
</script>

<CheckboxPrimitive.Root
	bind:ref
	data-slot="checkbox"
	class={cn(
		"bg-input/90 data-checked:bg-primary data-checked:text-primary-foreground dark:data-checked:bg-primary data-checked:border-primary border-transparent aria-invalid:aria-checked:border-primary aria-invalid:border-destructive dark:aria-invalid:border-destructive/50 focus-visible:border-ring focus-visible:ring-ring aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 flex size-4 items-center justify-center rounded-[5px] border transition-shadow group-has-disabled/field:opacity-50 focus-visible:ring-2 aria-invalid:ring-3 group-has-[:focus-visible]/field-label:ring-0 group-has-[:focus-visible]/field-label:not-data-checked:border-transparent group-has-[:focus-visible]/field-label:data-checked:border-primary peer relative shrink-0 outline-none after:absolute after:-inset-x-3 after:-inset-y-2 disabled:cursor-not-allowed disabled:opacity-50",
		className
	)}
	bind:checked
	{...restProps}
>
	{#snippet children({ checked })}
		<div
			data-slot="checkbox-indicator"
			class="[&>svg]:size-3.5 grid place-content-center text-current transition-none"
		>
			<!-- No indeterminate state: shadcn's minus icon is removed, since no checkbox in the app is ever indeterminate. -->
			{#if checked}
				<CheckIcon  />
			{/if}
		</div>
	{/snippet}
</CheckboxPrimitive.Root>
