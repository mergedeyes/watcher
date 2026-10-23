<script lang="ts">
	import { page } from '$app/state';
	import { api, ApiError, HISTORY_DELAY, hostHref, type ServiceDetail, type StateEvent } from '#lib/api.ts';
	import AlertTable from '#lib/AlertTable.svelte';
	import { auth } from '#lib/auth.svelte.ts';
	import { editHref } from '#lib/config.svelte.ts';
	import ConfigTree from '#lib/ConfigTree.svelte';
	import EditMenu, { type EditTarget } from '#lib/EditMenu.svelte';
	import Commands from '#lib/Commands.svelte';
	import Flags from '#lib/Flags.svelte';
	import Graphs from '#lib/Graphs.svelte';
	import { datetime, duration, formatThreshold, formatValue, seriesName, since } from '#lib/format.ts';
	import Status from '#lib/Status.svelte';
	import { live } from '#lib/live.svelte.ts';

	let host = $derived(page.params.host ?? '');
	let name = $derived(page.params.service ?? '');

	let detail = $state<ServiceDetail | null>(null);
	let missing = $state(false);
	let events = $state<StateEvent[]>([]);
	/** Set when the hub has no database: there is no history to show. */
	let noHistory = $state(false);

	/** Where the service can be edited: the service, the host (override or one-off service), its check. */
	function editTargets(d: ServiceDetail): EditTarget[] {
		const focus = (s: string) => `&focus=${encodeURIComponent(s)}`;
		const out: EditTarget[] = [];
		if (d.check.service)
			out.push({ label: `Service ${d.check.service}`, href: editHref('services', d.check.service), hint: 'all hosts with it' });
		out.push({
			label: `Host ${d.host}`,
			href: editHref('hosts', d.host) + focus(d.service),
			hint: d.check.service ? 'override on this host' : 'service on this host'
		});
		if (d.check.name) out.push({ label: `Check ${d.check.name}`, href: editHref('checks', d.check.name), hint: 'command, thresholds' });
		return out;
	}

	// Refetch whenever a new result for this service arrives, and after
	// commands (the server then sends a reload).
	let lastCheck = $derived(live.find(host, name)?.last_check ?? null);

	$effect(() => {
		const [h, s, _, __] = [host, name, lastCheck, live.reloads];
		api.service(h, s).then(
			(d) => {
				detail = d;
				missing = false;
			},
			(e) => {
				if (e instanceof ApiError && e.status === 404) missing = true;
			}
		);
	});

	// History reaches the database about a second after a result: fetch
	// now and again once it is written.
	$effect(() => {
		const [h, s, _] = [host, name, lastCheck];
		const load = () =>
			api.events({ host: h, service: s, type: 'service,flapping', limit: 20 }).then(
				(ev) => (events = ev),
				(e) => (noHistory = e instanceof ApiError && e.noDatabase)
			);
		load();
		const later = setTimeout(load, HISTORY_DELAY);
		return () => clearTimeout(later);
	});
</script>

<!-- Evaluated threshold, marked `cfg` if it isn't the plugin's; a plugin
     threshold that wasn't evaluated is shown muted. -->
{#snippet threshold(evaluated: string | undefined, plugin: string | undefined, unit: string | undefined)}
	{#if evaluated}
		{formatThreshold(evaluated, unit)}
		{#if evaluated !== plugin}<span class="tag" title="From config">cfg</span>{/if}
	{/if}
	{#if plugin && evaluated !== plugin}
		<span class="muted" title="From plugin, not evaluated">{formatThreshold(plugin, unit)}</span>
	{/if}
{/snippet}

<div class="content">
	{#if missing}
		<p class="notice">Service not found: {host}/{name}</p>
	{:else if !detail}
		<p class="notice">Loading…</p>
	{:else}
		<div class="page-head">
			<h1 title={detail.title ? detail.service : undefined}>{detail.title ?? detail.service} <span class="muted">on <a href={hostHref(host)}>{host}</a></span></h1>
			{#if auth.admin}
				<span class="head-actions">
					<EditMenu targets={editTargets(detail)} />
					<ConfigTree kind="service" name="{host}/{name}" />
				</span>
			{/if}
		</div>

		<section class="panel">
			<div class="panel-title">Service State Information</div>
			<table class="grid kv">
				<tbody>
					<tr>
						<th>Current Status</th>
						<td>
							<Status state={detail.state} />
							{#if detail.last_change}<span class="muted">&nbsp;for {since(detail.last_change, live.now)}</span>{/if}
							{#if detail.flapping}<span class="tag flap">FLAPPING</span>{/if}
							<Flags v={detail} />
						</td>
					</tr>
					<tr><th>Status Information</th><td class="info pre">{detail.output}{detail.long_output ? `\n${detail.long_output}` : ''}</td></tr>
					<tr>
						<th>Current Attempt</th>
						<td>
							{#if detail.attempt !== null}{detail.attempt}/{detail.max_attempts} ({detail.state_type?.toUpperCase()} state){:else}–{/if}
						</td>
					</tr>
					<tr><th>Last Check Time</th><td>{datetime(detail.last_check) || '–'}</td></tr>
					<tr><th>Check Type</th><td>{detail.passive ? 'PASSIVE' : 'ACTIVE'}</td></tr>
					{#if detail.duration_ms !== null}
						<tr><th>Check Duration</th><td>{formatValue(detail.duration_ms / 1000, 's')}</td></tr>
					{/if}
					<tr><th>Last State Change</th><td>{datetime(detail.last_hard_change) || '–'}</td></tr>
					<tr>
						<th>Flap Detection</th>
						<td>
							{#if detail.check.flap_high === null}
								off
							{:else}
								{detail.flapping ? 'FLAPPING' : 'not flapping'}, {(detail.percent_change ?? 0).toFixed(1)}% state change
								<span class="muted">(start {detail.check.flap_high}%, stop {detail.check.flap_low}%)</span>
							{/if}
						</td>
					</tr>
					{#if detail.host_check}<tr><th>Host Check</th><td>gives {detail.host} UP/DOWN</td></tr>{/if}
					{#if detail.check.command}
						<tr>
							<th>Check Interval</th>
							<td>{duration(detail.check.interval_s)}, retry {duration(detail.check.retry_interval_s)}, timeout {duration(detail.check.timeout_s)}</td>
						</tr>
						<tr><th>Check Command</th><td class="mono info">{detail.check.command}</td></tr>
					{:else if detail.check.freshness_s !== null}
						<tr><th>Freshness Threshold</th><td>{duration(detail.check.freshness_s)}</td></tr>
					{/if}
					<tr><th>Check</th><td>{detail.check.name ?? (detail.check.command ? 'inline command' : 'passive')}</td></tr>
					{#if detail.check.assigned.length}
						<tr><th>Assigned By</th><td>{detail.check.assigned.join('; ')}</td></tr>
					{/if}
					{#if detail.groups.length}
						<tr>
							<th>Service Groups</th>
							<td>{#each detail.groups as g, i (i)}{i ? ', ' : ''}<a href="/services?group={encodeURIComponent(g)}">{g}</a>{/each}</td>
						</tr>
					{/if}
					<tr><th>Source</th><td class="mono">{detail.check.origin}</td></tr>
				</tbody>
			</table>
		</section>

		<Commands target={{ service: detail }} flags={detail.flags} downtimes={detail.downtimes} />

		{#if detail.metrics.length > 0}
			<section class="panel">
				<div class="panel-title">Performance Data</div>
				<table class="grid">
					<thead>
						<tr>
							<th>Metric</th>
							<th>Value</th>
							<th class="hide-narrow">Warning</th>
							<th class="hide-narrow">Critical</th>
							<th>Status</th>
						</tr>
					</thead>
					<tbody>
						{#each detail.metrics as m, i (i)}
							<tr data-state={m.state}>
								<td class="mono">{seriesName(m.name, m.labels)}</td>
								<td class="mono nowrap">{formatValue(m.value, m.unit)}</td>
								<td class="mono hide-narrow">{@render threshold(m.warn, m.perf_warn, m.unit)}</td>
								<td class="mono hide-narrow">{@render threshold(m.crit, m.perf_crit, m.unit)}</td>
								<td>{#if m.state}<Status state={m.state} />{/if}</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</section>
		{/if}

		{#if detail.graphs}
			<Graphs {host} service={name} tick={Date.parse(lastCheck ?? '') || 0} />
		{/if}

		{#if !noHistory}
			<section class="panel">
				<div class="panel-title">
					Alert History
					<a class="more" href="/alerts?host={encodeURIComponent(host)}&service={encodeURIComponent(name)}">All</a>
				</div>
				<AlertTable {events} empty="No state changes." />
			</section>
		{/if}
	{/if}
</div>

<style>
	h1 .muted {
		font-weight: 400;
		font-size: 14px;
	}

	.kv th {
		width: 170px;
		background: var(--panel-alt);
		vertical-align: top;
		border-bottom: 1px solid var(--line);
	}

	.pre {
		white-space: pre-wrap;
	}

	.head-actions {
		display: inline-flex;
		align-items: center;
		gap: 10px;
	}

	.more {
		font-weight: 400;
		font-size: 12px;
	}
</style>
