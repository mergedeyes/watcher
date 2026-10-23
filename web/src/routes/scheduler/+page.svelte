<script lang="ts">
	import { api, hostHref, serviceHref, type Scheduled } from '#lib/api.ts';
	import { actions } from '#lib/actions.svelte.ts';
	import { datetime, duration } from '#lib/format.ts';
	import { live } from '#lib/live.svelte.ts';

	let queue = $state<Scheduled[]>([]);
	let query = $state('');
	let error = $state<string | null>(null);

	// Reload with the clock and after reloads or commands.
	$effect(() => {
		void live.now;
		void live.reloads;
		api.scheduler(2000).then(
			(q) => {
				queue = q;
				error = null;
			},
			(e) => (error = e instanceof Error ? e.message : String(e))
		);
	});

	let shown = $derived.by(() => {
		const q = query.trim().toLowerCase();
		return queue.filter((j) => !q || `${j.host}/${j.service}`.toLowerCase().includes(q));
	});

	/** "in 12s", or the time when it is further away. */
	function next(t: string | null): string {
		if (!t) return '';
		const secs = Math.round((Date.parse(t) - live.now) / 1000);
		if (secs <= 0) return 'now';
		return secs < 3600 ? `in ${duration(secs)}` : datetime(t);
	}

	function menu(e: MouseEvent, j: Scheduled) {
		const s = live.find(j.host, j.service);
		if (s) actions.open(e, { service: s });
	}
</script>

<div class="content">
	<div class="page-head">
		<h1>Scheduling Queue</h1>
		<input type="search" placeholder="Search host, service" bind:value={query} aria-label="Search" />
	</div>
	{#if error}<p class="notice error">{error}</p>{/if}
	<section class="panel">
		<div class="panel-title">Checks <span class="muted">{shown.length}</span></div>
		{#if shown.length === 0}
			<div class="empty">No checks.</div>
		{:else}
			<table class="grid">
				<thead>
					<tr>
						<th>Host</th>
						<th>Service</th>
						<th>Last Check</th>
						<th>Next Check</th>
						<th class="hide-narrow">Interval</th>
						<th>Type</th>
					</tr>
				</thead>
				<tbody>
					{#each shown as j (`${j.host}/${j.service}`)}
						<tr oncontextmenu={(e) => menu(e, j)}>
							<td class="nowrap"><a href={hostHref(j.host)}>{j.host}</a></td>
							<td class="nowrap"><a href={serviceHref(j.host, j.service)}>{j.service}</a></td>
							<td class="nowrap">{datetime(j.last_check)}</td>
							<td class="nowrap">{j.running ? 'running' : next(j.next_check)}</td>
							<td class="nowrap hide-narrow">{j.passive ? '' : duration(j.interval_s)}</td>
							<td class="nowrap">
								{#if j.passive}<span class="tag">passive{j.next_check ? ', freshness' : ''}</span>{/if}
								{#if j.paused}<span class="tag off">CHECKS OFF</span>{/if}
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		{/if}
	</section>
</div>

<style>
	.error {
		color: var(--crit);
	}
</style>
