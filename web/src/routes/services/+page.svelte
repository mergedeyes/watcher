<script lang="ts">
	import { page } from '$app/state';
	import type { Counts } from '#lib/api.ts';
	import { auth } from '#lib/auth.svelte.ts';
	import { editHref } from '#lib/config.svelte.ts';
	import ConfigTree from '#lib/ConfigTree.svelte';
	import { Debounced } from '#lib/list.svelte.ts';
	import ServicePanel from '#lib/ServicePanel.svelte';
	import Totals from '#lib/Totals.svelte';

	const STATES = ['ok', 'warning', 'critical', 'unknown', 'pending'] as const;
	type Filter = (typeof STATES)[number];

	let filter = $derived.by(() => {
		const s = page.url.searchParams.get('state');
		return (STATES as readonly string[]).includes(s ?? '') ? (s as Filter) : null;
	});
	const search = new Debounced();
	/** Only services of this service group. */
	let group = $derived(page.url.searchParams.get('group'));

	/** Per state, of the services the group and search allow. */
	let counts = $state<Counts | null>(null);
	let all = $derived(counts ? Object.values(counts).reduce((a, b) => a + b, 0) : 0);

	/** Totals links keep the group. */
	function href(state?: string) {
		const params = new URLSearchParams();
		if (group) params.set('group', group);
		if (state) params.set('state', state);
		const q = params.toString();
		return q ? `/services?${q}` : '/services';
	}
</script>

<div class="content">
	<div class="page-head">
		<h1>Services{group ? ` · ${group}` : ''}{filter ? ` · ${filter.toUpperCase()}` : ''}</h1>
		<span class="head-actions">
			{#if group && auth.admin}
				<a class="btn" href={editHref('servicegroups', group)}>Edit Config</a>
				<ConfigTree kind="servicegroup" name={group} />
			{/if}
			<input type="search" placeholder="Search host, service, output" bind:value={search.value} aria-label="Search" />
		</span>
	</div>

	<div class="totals">
		<Totals
			title="Service Status Totals"
			items={[
				{ key: 'all', label: 'All', count: all, href: href() },
				{ key: 'ok', label: 'OK', count: counts?.ok ?? 0, href: href('ok'), state: 'ok' },
				{ key: 'warning', label: 'Warning', count: counts?.warning ?? 0, href: href('warning'), state: 'warning' },
				{ key: 'critical', label: 'Critical', count: counts?.critical ?? 0, href: href('critical'), state: 'critical' },
				{ key: 'unknown', label: 'Unknown', count: counts?.unknown ?? 0, href: href('unknown'), state: 'unknown' },
				{ key: 'pending', label: 'Pending', count: counts?.pending ?? 0, href: href('pending') }
			]}
		/>
	</div>

	<ServicePanel
		title="Service Status Details"
		filter={{ state: filter ?? undefined, group: group ?? undefined, q: search.settled || undefined }}
		none={group || search.settled ? undefined : 'No services configured.'}
		empty="No matches."
		onlist={(l) => (counts = l.counts)}
	/>
</div>

<style>
	.totals {
		margin-bottom: 16px;
	}
</style>
