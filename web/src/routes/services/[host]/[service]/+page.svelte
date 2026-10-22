<script lang="ts">
	import { page } from '$app/state';
	import { api, ApiError, hostHref, type Series, type ServiceDetail, type StateEvent } from '#lib/api.ts';
	import Chart from '#lib/Chart.svelte';
	import { datetime, duration, formatThreshold, formatValue, seriesName, since } from '#lib/format.ts';
	import Status from '#lib/Status.svelte';
	import { live } from '#lib/live.svelte.ts';

	let host = $derived(page.params.host ?? '');
	let name = $derived(page.params.service ?? '');

	let detail = $state<ServiceDetail | null>(null);
	let missing = $state(false);
	let series = $state<Series[]>([]);
	let events = $state<StateEvent[]>([]);
	/** Set when the hub has no database: there is no history to show. */
	let noHistory = $state(false);
	/** Time ranges for the graphs, in seconds. */
	const RANGES: [string, number][] = [
		['30s', 30],
		['1m', 60],
		['5m', 300],
		['15m', 900],
		['30m', 1800],
		['1h', 3600],
		['6h', 21600],
		['24h', 86400],
		['7d', 604800],
		['30d', 2592000]
	];
	const DEFAULT_MAX = 21600;

	/** Range picked by the user; null means automatic. */
	let chosen = $state<string | null>(null);

	/** When the oldest sample of this service was taken (ms), null without data. */
	let oldest = $derived.by(() => {
		const times = series.map((s) => (s.first ? Date.parse(s.first) : NaN)).filter((t) => !isNaN(t));
		return times.length > 0 ? Math.min(...times) : null;
	});

	// The smallest range holding all data. Larger ones would show the same
	// samples in coarser buckets, so they are disabled.
	let cover = $derived.by(() => {
		const span = oldest === null ? 0 : (live.now - oldest) / 1000;
		return RANGES.find(([, secs]) => secs >= span) ?? RANGES[RANGES.length - 1]!;
	});

	function enabled(secs: number) {
		return oldest !== null && secs <= cover[1];
	}

	let range = $derived.by(() => {
		const pick = RANGES.find(([label]) => label === chosen);
		if (pick && enabled(pick[1])) return pick;
		// Automatic: all data, but no more than 6h.
		return cover[1] <= DEFAULT_MAX ? cover : RANGES.find(([, secs]) => secs === DEFAULT_MAX)!;
	});

	// Refetch whenever a new result for this service arrives.
	let lastCheck = $derived(live.find(host, name)?.last_check ?? null);

	$effect(() => {
		const [h, s, _] = [host, name, lastCheck];
		api.service(h, s).then(
			(d) => {
				detail = d;
				missing = false;
			},
			(e) => {
				if (e instanceof ApiError && e.status === 404) missing = true;
			}
		);
		api.events(h, s).then(
			(ev) => (events = ev),
			(e) => (noHistory = e instanceof ApiError && e.noDatabase)
		);
	});

	$effect(() => {
		const [h, s] = [host, name];
		api.series(h, s).then(
			(list) => (series = list),
			(e) => (noHistory = e instanceof ApiError && e.noDatabase)
		);
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
			<h1>{detail.service} <span class="muted">on <a href={hostHref(host)}>{host}</a></span></h1>
		</div>

		<section class="panel">
			<div class="panel-title">Service State Information</div>
			<table class="grid kv">
				<tbody>
					<tr>
						<th>Current Status</th>
						<td>
							<Status state={detail.state} />
							{#if detail.flapping}<span class="tag flap">FLAPPING</span>{/if}
							{#if detail.last_change}<span class="muted">&nbsp;for {since(detail.last_change, live.now)}</span>{/if}
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
					<tr><th>Source</th><td class="mono">{detail.check.origin}</td></tr>
				</tbody>
			</table>
		</section>

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
						{#each detail.metrics as m (seriesName(m.name, m.labels))}
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

		<section class="panel">
			<div class="panel-title">
				Graphs
				{#if !noHistory && series.length > 0}
					<span class="ranges" role="group" aria-label="Time range">
						{#each RANGES as [label, secs] (label)}
							<button
								type="button"
								aria-pressed={range[0] === label}
								disabled={!enabled(secs)}
								title={enabled(secs) ? undefined : 'Not enough data'}
								onclick={() => (chosen = label)}>{label}</button
							>
						{/each}
					</span>
				{/if}
			</div>
			{#if noHistory}
				<div class="empty">No database configured.</div>
			{:else if series.length === 0}
				<div class="empty">No performance data.</div>
			{:else}
				<div class="graphs">
					{#each series as s (s.id)}
						<Chart series={s} range={range[0]} rangeSecs={range[1]} tick={Date.parse(lastCheck ?? '') || 0} />
					{/each}
				</div>
			{/if}
		</section>

		{#if !noHistory}
			<section class="panel">
				<div class="panel-title">Alert History</div>
				{#if events.length === 0}
					<div class="empty">No state changes.</div>
				{:else}
					<table class="grid">
						<thead>
							<tr>
								<th>Time</th>
								<th>Status</th>
								<th class="hide-narrow">Type</th>
								<th>Status Information</th>
							</tr>
						</thead>
						<tbody>
							{#each events as e (e.ts)}
								<tr>
									<td class="nowrap">{datetime(e.ts)}</td>
									<td><Status state={e.to_state} /></td>
									<td class="hide-narrow">{e.state_type.toUpperCase()}</td>
									<td class="info">{e.output}</td>
								</tr>
							{/each}
						</tbody>
					</table>
				{/if}
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

	.ranges {
		display: inline-flex;
		flex-wrap: wrap;
		gap: 2px;
	}

	.ranges button {
		font: inherit;
		font-size: 11px;
		font-weight: 600;
		padding: 2px 7px;
		color: var(--muted);
		background: var(--panel);
		border: 1px solid var(--line);
		border-radius: 3px;
		cursor: pointer;
	}

	.ranges button[aria-pressed='true'] {
		background: var(--link);
		border-color: var(--link);
		color: #fff;
	}

	.ranges button:disabled {
		opacity: 0.35;
		cursor: not-allowed;
	}

	.graphs {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(min(100%, 480px), 1fr));
		gap: 12px;
		padding: 12px;
	}
</style>
