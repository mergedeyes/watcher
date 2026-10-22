<script lang="ts">
	import { page } from '$app/state';
	import { api, ApiError, HISTORY_DELAY, serviceHref, type StateEvent } from '#lib/api.ts';
	import AlertTable from '#lib/AlertTable.svelte';
	import { datetime, since } from '#lib/format.ts';
	import { live } from '#lib/live.svelte.ts';
	import ServiceTable from '#lib/ServiceTable.svelte';
	import Status from '#lib/Status.svelte';

	let name = $derived(page.params.host ?? '');
	let host = $derived(live.hosts.find((h) => h.name === name));
	let services = $derived(live.services.filter((s) => s.host === name));

	let origin = $state<string | null>(null);
	let missing = $state(false);
	let events = $state<StateEvent[]>([]);
	/** Set when the hub has no database: there is no history to show. */
	let noHistory = $state(false);

	// Refetch when the host check reports a change, and again once the
	// change is in the database.
	let lastChange = $derived(host?.last_change ?? null);
	$effect(() => {
		const [current, _] = [name, lastChange];
		const load = () =>
			api.events({ host: current, type: 'host', limit: 20 }).then(
				(ev) => (events = ev),
				(e) => (noHistory = e instanceof ApiError && e.noDatabase)
			);
		load();
		const later = setTimeout(load, HISTORY_DELAY);
		return () => clearTimeout(later);
	});

	$effect(() => {
		const current = name;
		missing = false;
		api.host(current).then(
			(r) => (origin = r.origin),
			(e) => {
				if (e instanceof ApiError && e.status === 404) missing = true;
			}
		);
	});
</script>

<div class="content">
	{#if missing}
		<p class="notice">Host not found: {name}</p>
	{:else if !host}
		<p class="notice">Loading…</p>
	{:else}
		<div class="page-head">
			<h1>{host.name}</h1>
			<Status state={host.state} />
		</div>

		<section class="panel">
			<div class="panel-title">Host State Information</div>
			<table class="grid kv">
				<tbody>
					<tr>
						<th>Host Status</th>
						<td>
							<Status state={host.state} />
							{#if host.flapping}<span class="tag flap">FLAPPING</span>{/if}
							{#if host.last_change}<span class="muted">&nbsp;for {since(host.last_change, live.now)}</span>{/if}
						</td>
					</tr>
					{#if host.check}
						<tr><th>Status Information</th><td class="info">{host.output || '–'}</td></tr>
						<tr>
							<th>Current Attempt</th>
							<td>
								{#if host.attempt !== null}{host.attempt}/{host.max_attempts} ({host.state_type?.toUpperCase()} state){:else}–{/if}
							</td>
						</tr>
						<tr><th>Last Check Time</th><td>{datetime(host.last_check) || '–'}</td></tr>
						<tr><th>Host Check</th><td><a href={serviceHref(host.name, host.check)}>{host.check}</a></td></tr>
					{:else}
						<tr><th>Host Check</th><td class="muted">none (assumed UP)</td></tr>
					{/if}
				</tbody>
			</table>
		</section>

		<section class="panel">
			<div class="panel-title">Host Information</div>
			<table class="grid kv">
				<tbody>
					<tr><th>Address</th><td class="mono">{host.address}</td></tr>
					{#if host.node}<tr><th>Node</th><td>{host.node}</td></tr>{/if}
					<tr><th>Tags</th><td>{#each host.tags as t (t)}<span class="tag">{t}</span> {/each}</td></tr>
					{#each Object.entries(host.vars) as [k, v] (k)}
						<tr><th>vars.{k}</th><td class="mono">{v}</td></tr>
					{/each}
					{#if !host.enabled}<tr><th>Enabled</th><td>no</td></tr>{/if}
					{#if origin}<tr><th>Source</th><td class="mono">{origin}</td></tr>{/if}
				</tbody>
			</table>
		</section>

		<section class="panel">
			<div class="panel-title">Services <span class="muted">{services.length}</span></div>
			<ServiceTable rows={services} showHost={false} sort="service" />
		</section>

		{#if !noHistory}
			<section class="panel">
				<div class="panel-title">
					Alert History
					<a class="more" href="/alerts?host={encodeURIComponent(name)}">All</a>
				</div>
				<AlertTable {events} empty={host.check ? 'No state changes.' : 'No host check.'} />
			</section>
		{/if}
	{/if}
</div>

<style>
	.more {
		font-weight: 400;
		font-size: 12px;
	}

	.kv th {
		width: 160px;
		background: var(--panel-alt);
		vertical-align: top;
		border-bottom: 1px solid var(--line);
	}
</style>
