<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api, ApiError, HISTORY_DELAY, serviceHref, type Flag, type StateEvent } from '#lib/api.ts';
	import AlertTable from '#lib/AlertTable.svelte';
	import { auth } from '#lib/auth.svelte.ts';
	import { editHref } from '#lib/config.svelte.ts';
	import Commands from '#lib/Commands.svelte';
	import Flags from '#lib/Flags.svelte';
	import ConfigTree from '#lib/ConfigTree.svelte';
	import EditMenu from '#lib/EditMenu.svelte';
	import Graphs from '#lib/Graphs.svelte';
	import { datetime, since } from '#lib/format.ts';
	import { live } from '#lib/live.svelte.ts';
	import ServiceTable from '#lib/ServiceTable.svelte';
	import Status from '#lib/Status.svelte';
	import VarTree from '#lib/VarTree.svelte';

	let name = $derived(page.params.host ?? '');
	let host = $derived(live.hosts.find((h) => h.name === name));
	let services = $derived(live.services.filter((s) => s.host === name));

	/** The service whose graphs show: picked (`?graph=`), or the host's `graph`. */
	let picked = $derived(page.url.searchParams.get('graph') ?? host?.graph ?? null);
	/** Null when that service has graphs turned off. */
	let graph = $derived(picked && live.find(name, picked)?.graphs !== false ? picked : null);
	let graphCheck = $derived(graph ? (live.find(name, graph)?.last_check ?? null) : null);

	function pick(service: string) {
		const params = new URLSearchParams(page.url.search);
		if (service === host?.graph) params.delete('graph');
		else params.set('graph', service);
		const query = params.toString();
		goto(query ? `?${query}` : page.url.pathname, { replace: true, reset: false });
	}

	let origin = $state<string | null>(null);
	let flags = $state<Flag[]>([]);
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
			// Host UP/DOWN, and acknowledgements and downtimes of the host itself.
			api.events({ host: current, type: 'host,ack,downtime', limit: 40 }).then(
				(ev) => (events = ev.filter((e) => e.kind === 'host' || e.service === '' || e.service === host?.check).slice(0, 20)),
				(e) => (noHistory = e instanceof ApiError && e.noDatabase)
			);
		load();
		const later = setTimeout(load, HISTORY_DELAY);
		return () => clearTimeout(later);
	});

	$effect(() => {
		const [current, _] = [name, live.reloads];
		missing = false;
		api.host(current).then(
			(r) => {
				origin = r.origin;
				flags = r.flags;
			},
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
			<span class="head-actions">
				{#if auth.admin}
					<EditMenu
						targets={[
							{ label: `Host ${host.name}`, href: editHref('hosts', host.name) },
							...host.groups.map((g) => ({ label: `Host Group ${g}`, href: editHref('hostgroups', g), hint: 'all hosts in the group' }))
						]}
					/>
					<ConfigTree kind="host" name={host.name} />
				{/if}
				<Status state={host.state} />
			</span>
		</div>

		<section class="panel">
			<div class="panel-title">Host State Information</div>
			<table class="grid kv">
				<tbody>
					<tr>
						<th>Host Status</th>
						<td>
							<Status state={host.state} />
							{#if host.last_change}<span class="muted">&nbsp;for {since(host.last_change, live.now)}</span>{/if}
							{#if host.flapping}<span class="tag flap">FLAPPING</span>{/if}
							<Flags v={host} />
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
					{#if host.ssh}
						<tr>
							<th>SSH</th>
							<td class="mono">{host.ssh.user}@{host.address}{host.ssh.port === 22 ? '' : `:${host.ssh.port}`} <span class="muted">secret {host.ssh.secret}</span></td>
						</tr>
					{/if}
					{#if host.groups.length}
						<tr>
							<th>Host Groups</th>
							<td>{#each host.groups as g, i (i)}{i ? ', ' : ''}<a href="/hosts?group={encodeURIComponent(g)}">{g}</a>{/each}</td>
						</tr>
					{/if}
					{#each Object.entries(host.vars) as [k, v] (k)}
						<tr><th>vars.{k}</th><td class="mono"><VarTree {v} /></td></tr>
					{/each}
					{#if !host.enabled}<tr><th>Enabled</th><td>no</td></tr>{/if}
					{#if origin}<tr><th>Source</th><td class="mono">{origin}</td></tr>{/if}
				</tbody>
			</table>
		</section>

		<Commands target={{ host }} {flags} />

		<section class="panel">
			<div class="panel-title">Services <span class="muted">{services.length}</span></div>
			<ServiceTable rows={services} showHost={false} sort="service" />
		</section>

		{#snippet picker()}
			<select class="pick" aria-label="Service" value={graph ?? ''} onchange={(e) => pick(e.currentTarget.value)}>
				{#if !graph}<option value="">–</option>{/if}
				{#each services.filter((s) => s.graphs).map((s) => s.service).sort() as s (s)}<option value={s}>{s}</option>{/each}
			</select>
		{/snippet}
		{#if graph}
			<Graphs host={name} service={graph} tick={Date.parse(graphCheck ?? '') || 0} controls={picker} />
		{:else}
			<section class="panel">
				<div class="panel-title">Graphs {@render picker()}</div>
				<div class="empty">No graph.</div>
			</section>
		{/if}

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

	.pick {
		font: inherit;
		font-weight: 400;
		font-size: 12px;
		color: var(--text);
		background: var(--panel);
		border: 1px solid var(--line);
		border-radius: 3px;
		padding: 1px 4px;
		margin-right: auto;
	}

	.kv th {
		width: 160px;
		background: var(--panel-alt);
		vertical-align: top;
		border-bottom: 1px solid var(--line);
	}
</style>
