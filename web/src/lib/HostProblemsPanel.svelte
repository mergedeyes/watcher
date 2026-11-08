<!-- Hosts DOWN, paged on the server and kept live from their host checks. -->
<script lang="ts">
	import { api } from './api.ts';
	import HostProblems from './HostProblems.svelte';
	import { Paging, ServerList, checkId, patchHost } from './list.svelte.ts';
	import Pager from './Pager.svelte';

	let { title, hideEmpty = false }: { title: string; hideEmpty?: boolean } = $props();

	const paging = new Paging(() => '');
	const list = new ServerList(
		() => ({ state: 'down', sort: 'host' as const, offset: paging.offset, limit: paging.limit }),
		api.hosts,
		{
			watch: (d) => ({ services: d.hosts.map(checkId).filter((id) => id !== null) }),
			patch: (d, view) => {
				for (const h of d.hosts) patchHost(h, view);
			}
		}
	);
</script>

{#if !(hideEmpty && (list.data?.total ?? 0) === 0)}
	<section class="panel">
		<div class="panel-title">{title} {#if list.data}<span class="muted">{list.data.total}</span>{/if}</div>
		{#if list.error && !list.data}
			<div class="empty error">{list.error}</div>
		{:else if !list.data}
			<div class="empty">Loading…</div>
		{:else if list.data.total === 0}
			<div class="empty">No hosts DOWN.</div>
		{:else}
			<HostProblems hosts={list.data.hosts} />
			<Pager {paging} total={list.data.total} />
		{/if}
	</section>
{/if}
