<!--
	A panel with a service table paged, filtered and sorted on the server,
	kept live: rows on the page are patched from results, the page is
	fetched again after state changes.
-->
<script lang="ts">
	import { count } from './format.ts';
	import { untrack, type Snippet } from 'svelte';
	import { api, type ServiceList, type ServiceQuery } from './api.ts';
	import { Paging, ServerList, patchRow, sid } from './list.svelte.ts';
	import Pager from './Pager.svelte';
	import ServiceTable, { type ServiceSortKey } from './ServiceTable.svelte';
	import { Sorter } from './sort.svelte.ts';

	interface Props {
		title: string;
		/** Filters; sort and page are the panel's. */
		filter?: ServiceQuery;
		/** Initial sort column. */
		sort?: ServiceSortKey;
		showHost?: boolean;
		/** Without any services at all. */
		none?: string;
		/** With services, but none match. */
		empty?: string;
		/** Gets every fetched list, e.g. for status totals. */
		onlist?: (list: ServiceList) => void;
		/** More in the title bar, on the right. */
		actions?: Snippet;
	}

	let { title, filter = {}, sort = 'host', showHost = true, none, empty = 'No services.', onlist, actions }: Props = $props();

	const sorter = untrack(() => new Sorter<ServiceSortKey>(sort, sort === 'status'));
	const paging = new Paging(() => JSON.stringify([filter, sorter.key, sorter.desc]));

	const list = new ServerList(
		() => ({ ...filter, sort: sorter.key, desc: sorter.desc, offset: paging.offset, limit: paging.limit }),
		api.services,
		{
			watch: (d) => ({ services: d.services.map(sid) }),
			patch: (d, view) => patchRow(d.services, view)
		}
	);

	$effect(() => {
		if (list.data) onlist?.(list.data);
	});

	/** All services the filters allow before the state filter. */
	let all = $derived(list.data ? Object.values(list.data.counts).reduce((a, b) => a + b, 0) : 0);
</script>

<section class="panel">
	<div class="panel-title">
		{title}
		{#if list.data}<span class="muted">{count(list.data.total)}</span>{/if}
		{#if actions}<span class="right">{@render actions()}</span>{/if}
	</div>
	{#if list.error && !list.data}
		<div class="empty error">{list.error}</div>
	{:else if !list.data}
		<div class="empty">Loading…</div>
	{:else}
		<ServiceTable rows={list.data.services} {showHost} {sorter} empty={all === 0 && none ? none : empty} />
		<Pager {paging} total={list.data.total} />
	{/if}
</section>

<style>
	.right {
		margin-left: auto;
		font-weight: 400;
	}
</style>
