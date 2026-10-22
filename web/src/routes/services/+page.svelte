<script lang="ts">
	import { page } from '$app/state';
	import { live } from '#lib/live.svelte.ts';
	import ServiceTable from '#lib/ServiceTable.svelte';
	import Totals from '#lib/Totals.svelte';

	const STATES = ['ok', 'warning', 'critical', 'unknown', 'pending'] as const;
	type Filter = (typeof STATES)[number];

	let filter = $derived.by(() => {
		const s = page.url.searchParams.get('state');
		return (STATES as readonly string[]).includes(s ?? '') ? (s as Filter) : null;
	});
	let query = $state('');
	/** Only services of this service group. */
	let group = $derived(page.url.searchParams.get('group'));

	let counts = $derived.by(() => {
		const c = { ok: 0, warning: 0, critical: 0, unknown: 0, pending: 0 };
		for (const s of live.services) c[s.state ?? 'pending']++;
		return c;
	});

	let shown = $derived.by(() => {
		const q = query.trim().toLowerCase();
		return live.services.filter(
			(s) =>
				(filter === null || (s.state ?? 'pending') === filter) &&
				(!group || s.groups.includes(group)) &&
				(!q || `${s.host} ${s.service} ${s.output}`.toLowerCase().includes(q))
		);
	});
</script>

<div class="content">
	<div class="page-head">
		<h1>Services{group ? ` · ${group}` : ''}{filter ? ` · ${filter.toUpperCase()}` : ''}</h1>
		<input type="search" placeholder="Search host, service, output" bind:value={query} aria-label="Search" />
	</div>

	<div class="totals">
		<Totals
			title="Service Status Totals"
			items={[
				{ key: 'all', label: 'All', count: live.services.length, href: '/services' },
				{ key: 'ok', label: 'OK', count: counts.ok, href: '/services?state=ok', state: 'ok' },
				{ key: 'warning', label: 'Warning', count: counts.warning, href: '/services?state=warning', state: 'warning' },
				{ key: 'critical', label: 'Critical', count: counts.critical, href: '/services?state=critical', state: 'critical' },
				{ key: 'unknown', label: 'Unknown', count: counts.unknown, href: '/services?state=unknown', state: 'unknown' },
				{ key: 'pending', label: 'Pending', count: counts.pending, href: '/services?state=pending' }
			]}
		/>
	</div>

	{#if !live.loaded}
		<p class="notice">Loading…</p>
	{:else}
		<section class="panel">
			<div class="panel-title">Service Status Details <span class="muted">{shown.length}/{live.services.length}</span></div>
			<ServiceTable rows={shown} empty={live.services.length === 0 ? 'No services configured.' : 'No matches.'} />
		</section>
	{/if}
</div>

<style>
	.totals {
		margin-bottom: 16px;
	}
</style>
