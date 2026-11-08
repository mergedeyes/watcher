<script lang="ts">
	import { live } from '#lib/live.svelte.ts';
	import HostProblemsPanel from '#lib/HostProblemsPanel.svelte';
	import ServicePanel from '#lib/ServicePanel.svelte';
	import Totals from '#lib/Totals.svelte';

	let summary = $derived(live.summary);
</script>

<div class="content">
	<div class="page-head"><h1>Tactical Overview</h1></div>

	{#if !summary}
		<p class="notice">Loading…</p>
	{:else}
		<div class="totals">
			<Totals
				title="Host Status Totals"
				items={[
					{ key: 'total', label: 'Total', count: summary.hosts.total, href: '/hosts' },
					{ key: 'up', label: 'Up', count: summary.hosts.up, href: '/hosts?state=up', state: 'ok' },
					{ key: 'down', label: 'Down', count: summary.hosts.down, href: '/hosts?state=down', state: 'critical' },
					{ key: 'pending', label: 'Pending', count: summary.hosts.pending, href: '/hosts?state=pending' }
				]}
			/>
			<Totals
				title="Service Status Totals"
				items={[
					{ key: 'total', label: 'Total', count: summary.services.total, href: '/services' },
					{ key: 'ok', label: 'OK', count: summary.services.ok, href: '/services?state=ok', state: 'ok' },
					{ key: 'warning', label: 'Warning', count: summary.services.warning, href: '/services?state=warning', state: 'warning' },
					{ key: 'critical', label: 'Critical', count: summary.services.critical, href: '/services?state=critical', state: 'critical' },
					{ key: 'unknown', label: 'Unknown', count: summary.services.unknown, href: '/services?state=unknown', state: 'unknown' },
					{ key: 'pending', label: 'Pending', count: summary.services.pending, href: '/services?state=pending' }
				]}
			/>
		</div>

		<HostProblemsPanel title="Hosts DOWN" hideEmpty />
		<ServicePanel title="Service Problems" filter={{ problems: true }} sort="status" empty="No problems." />
	{/if}
</div>

<style>
	.totals {
		display: grid;
		grid-template-columns: minmax(0, 2fr) minmax(0, 3fr);
		gap: 16px;
		margin-bottom: 16px;
	}

	@media (max-width: 960px) {
		.totals {
			grid-template-columns: 1fr;
		}
	}
</style>
