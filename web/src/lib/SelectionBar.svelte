<!--
	A panel title that turns into a command bar while rows of its table are
	selected: how many, the commands that apply to any of them, and Clear.
	Taking the title's place keeps the table where it is, so the row under
	the pointer doesn't move when the first box is ticked.
-->
<script lang="ts">
	import type { Snippet } from 'svelte';
	import { actions, entries, id } from './actions.svelte.ts';
	import type { Selection } from './selection.svelte.ts';

	let { selection, children }: { selection?: Selection; children: Snippet } = $props();

	let targets = $derived(selection?.targets ?? []);
</script>

{#if selection && targets.length > 0}
	<div class="panel-title fixed bar">
		<span class="count">{targets.length} selected</span>
		<span class="commands">
			{#each entries(targets) as entry (entry.action)}
				<button
					type="button"
					class="btn"
					title={entry.targets.length < targets.length ? `${entry.label}: ${entry.targets.map(id).join(', ')}` : entry.label}
					onclick={() => actions.choose(entry, selection)}
				>
					{entry.short}{#if entry.targets.length < targets.length}&nbsp;<span class="n">({entry.targets.length})</span>{/if}{entry.dialog ? '…' : ''}
				</button>
			{/each}
		</span>
		<button type="button" class="btn clear" onclick={() => selection.clear()}>Clear</button>
	</div>
{:else}
	<div class="panel-title fixed">{@render children()}</div>
{/if}

<style>
	/* The bar is as tall as the plain title: the buttons take the padding's room. */
	.fixed {
		box-sizing: border-box;
		min-height: 36px;
	}

	.bar {
		justify-content: flex-start;
		flex-wrap: wrap;
		padding-block: 5px;
		background: color-mix(in srgb, var(--link) 10%, var(--panel));
	}

	.count {
		white-space: nowrap;
	}

	.commands {
		display: flex;
		flex-wrap: wrap;
		gap: 4px;
	}

	.btn {
		font-size: 12px;
		padding: 1px 8px;
	}

	.n {
		color: var(--muted);
	}

	.clear {
		margin-left: auto;
	}
</style>
