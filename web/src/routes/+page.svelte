<script lang="ts">
	import { live } from '#lib/live.svelte.ts';
	import ServiceTable from '#lib/ServiceTable.svelte';
	import Totals from '#lib/Totals.svelte';

	let problems = $derived(live.services.filter((s) => s.state !== null && s.state !== 'ok'));

	let services = $derived.by(() => {
		const c = { ok: 0, warning: 0, critical: 0, unknown: 0, pending: 0 };
		for (const s of live.services) c[s.state ?? 'pending']++;
		return c;
	});

	let hosts = $derived.by(() => {
		const c = { total: live.hosts.length, ok: 0, problems: 0, pending: 0 };
		for (const h of live.hosts) {
			if (h.state === null) c.pending++;
			else if (h.state === 'ok') c.ok++;
			else c.problems++;
		}
		return c;
	});
</script>

<div class="content">
	<div class="page-head"><h1>Tactical Overview</h1></div>

	{#if !live.loaded}
		<p class="notice">Loading…</p>
	{:else}
		<div class="totals">
			<Totals
				title="Host Status Totals"
				items={[
					{ key: 'total', label: 'Total', count: hosts.total, href: '/hosts' },
					{ key: 'ok', label: 'OK', count: hosts.ok, href: '/hosts?state=ok', state: 'ok' },
					{ key: 'problems', label: 'Problems', count: hosts.problems, href: '/hosts?state=problems', state: 'critical' },
					{ key: 'pending', label: 'Pending', count: hosts.pending, href: '/hosts?state=pending' }
				]}
			/>
			<Totals
				title="Service Status Totals"
				items={[
					{ key: 'total', label: 'Total', count: live.services.length, href: '/services' },
					{ key: 'ok', label: 'OK', count: services.ok, href: '/services?state=ok', state: 'ok' },
					{ key: 'warning', label: 'Warning', count: services.warning, href: '/services?state=warning', state: 'warning' },
					{ key: 'critical', label: 'Critical', count: services.critical, href: '/services?state=critical', state: 'critical' },
					{ key: 'unknown', label: 'Unknown', count: services.unknown, href: '/services?state=unknown', state: 'unknown' },
					{ key: 'pending', label: 'Pending', count: services.pending, href: '/services?state=pending' }
				]}
			/>
		</div>

		<section class="panel">
			<div class="panel-title">Unhandled Problems <span class="muted">{problems.length}</span></div>
			<ServiceTable rows={problems} sort="status" empty="No problems." />
		</section>
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
