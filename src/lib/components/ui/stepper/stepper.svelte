<script lang="ts" module>
	export interface StepperStep {
		id: string;
		label: string;
	}
</script>

<script lang="ts">
	/**
	 * A compact step indicator in the Rhea idiom (not part of the shadcn-svelte
	 * registry, which has no stepper): numbered dots joined by a rule, the
	 * current step marked with aria-current="step". Completed steps become
	 * buttons when `onSelect` is given, so a user can jump back.
	 */
	import CheckIcon from "@lucide/svelte/icons/check";
	import { cn } from "$lib/components/ui/utils.js";

	let {
		steps,
		current,
		onSelect,
		class: className,
		label = "Progress",
	}: {
		steps: readonly StepperStep[];
		/** Index of the current step. */
		current: number;
		/** Called with a step index when a completed step is clicked. */
		onSelect?: (index: number) => void;
		class?: string;
		label?: string;
	} = $props();
</script>

<nav aria-label={label} class={cn("w-full", className)} data-slot="stepper">
	<ol class="flex items-center gap-1.5">
		{#each steps as step, index (step.id)}
			{@const done = index < current}
			{@const active = index === current}
			<li class="flex min-w-0 flex-1 items-center gap-1.5" data-slot="stepper-item" data-state={active ? "active" : done ? "complete" : "upcoming"}>
				{#if done && onSelect}
					<button
						type="button"
						class="focus-visible:ring-ring/30 group/step flex min-w-0 items-center gap-1.5 rounded-2xl outline-none focus-visible:ring-3"
						onclick={() => onSelect(index)}
						aria-label={`${step.label}, completed. Go back to this step`}
					>
						<span class="bg-primary text-primary-foreground flex size-5 shrink-0 items-center justify-center rounded-full text-[11px]">
							<CheckIcon class="size-3" aria-hidden="true" />
						</span>
						<span class="text-muted-foreground group-hover/step:text-foreground truncate text-xs">{step.label}</span>
					</button>
				{:else}
					<span
						class={cn(
							"flex size-5 shrink-0 items-center justify-center rounded-full text-[11px] font-medium tabular-nums",
							active && "bg-primary text-primary-foreground",
							done && "bg-primary text-primary-foreground",
							!active && !done && "bg-muted text-muted-foreground"
						)}
						aria-hidden="true"
					>
						{#if done}<CheckIcon class="size-3" />{:else}{index + 1}{/if}
					</span>
					<span
						class={cn("truncate text-xs", active ? "text-foreground font-medium" : "text-muted-foreground")}
						aria-current={active ? "step" : undefined}
					>
						<span class="sr-only">Step {index + 1} of {steps.length}: </span>{step.label}
					</span>
				{/if}
				{#if index < steps.length - 1}
					<span class={cn("h-px min-w-2 flex-1", done ? "bg-primary" : "bg-border")} aria-hidden="true"></span>
				{/if}
			</li>
		{/each}
	</ol>
</nav>
