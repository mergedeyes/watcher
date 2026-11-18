<!--
	The checkbox of a table row, or without `target` the header's box for
	all rows on the page (half-ticked while only some are selected).
-->
<script lang="ts">
	import { id, type Target } from './actions.svelte.ts';
	import type { Selection } from './selection.svelte.ts';

	let { selection, target }: { selection: Selection; target?: Target } = $props();

	let checked = $derived(target ? selection.has(target) : selection.all);
	let some = $derived(!target && !selection.all && selection.targets.length > 0);
</script>

{#if target}
	<input
		type="checkbox"
		{checked}
		aria-label="Select {id(target)}"
		title="Select; shift-click selects a range"
		onclick={(e) => selection.click(target, e.shiftKey)}
	/>
{:else}
	<input type="checkbox" {checked} indeterminate={some} aria-label="Select all on this page" onclick={() => selection.setAll(!selection.all)} />
{/if}
