<!-- Page navigation under a server-paged table: rows shown, first/previous/next/last, page size. -->
<script lang="ts">
	import { count } from './format.ts';
	import { PAGE_SIZES, type Paging } from './list.svelte.ts';

	let { paging, total }: { paging: Paging; total: number } = $props();

	let offset = $derived(paging.offset);
	let limit = $derived(paging.limit);
	let pages = $derived(Math.max(1, Math.ceil(total / limit)));
	let current = $derived(Math.floor(offset / limit) + 1);
	let last = $derived((pages - 1) * limit);

	// Rows went away (e.g. problems recovered): show the last page there is.
	$effect(() => {
		if (offset > 0 && offset >= total) paging.go(last);
	});
</script>

{#if total > PAGE_SIZES[0] || offset > 0}
	<div class="pager">
		<span class="muted">{count(Math.min(offset + 1, total))}–{count(Math.min(offset + limit, total))} of {count(total)}</span>
		<span class="nav">
			<button type="button" class="btn" disabled={offset === 0} onclick={() => paging.go(0)} aria-label="First page">«</button>
			<button type="button" class="btn" disabled={offset === 0} onclick={() => paging.go(offset - limit)} aria-label="Previous page">‹</button>
			<span class="page">Page {count(current)} of {count(pages)}</span>
			<button type="button" class="btn" disabled={current >= pages} onclick={() => paging.go(offset + limit)} aria-label="Next page">›</button>
			<button type="button" class="btn" disabled={current >= pages} onclick={() => paging.go(last)} aria-label="Last page">»</button>
		</span>
		<label class="size">
			<select value={limit} onchange={(e) => paging.setLimit(Number(e.currentTarget.value))} aria-label="Rows per page">
				{#each PAGE_SIZES as n (n)}<option value={n}>{n}</option>{/each}
			</select>
			per page
		</label>
	</div>
{/if}

<style>
	.pager {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		justify-content: space-between;
		gap: 8px 16px;
		padding: 8px 10px;
		border-top: 1px solid var(--line);
		font-size: 12px;
	}

	.nav {
		display: inline-flex;
		align-items: center;
		gap: 4px;
	}

	.nav .btn {
		padding: 2px 8px;
		line-height: 1.3;
	}

	.page {
		padding: 0 6px;
		font-variant-numeric: tabular-nums;
	}

	.size {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		color: var(--muted);
	}

	.size select {
		font: inherit;
		color: var(--text);
		background: var(--panel);
		border: 1px solid var(--line);
		border-radius: 3px;
		padding: 1px 4px;
	}
</style>
