<script lang="ts">
	import { page } from '$app/state';
	import { api, ApiError, hostHref, type Series, type ServiceDetail, type StateEvent } from '#lib/api.ts';
	import Chart from '#lib/Chart.svelte';
	import { age, duration, formatThreshold, formatValue, seriesName, stateName, timestamp } from '#lib/format.ts';
	import Lamp from '#lib/Lamp.svelte';
	import { live } from '#lib/live.svelte.ts';

	let host = $derived(page.params.host ?? '');
	let name = $derived(page.params.service ?? '');

	let detail = $state<ServiceDetail | null>(null);
	let missing = $state(false);
	let series = $state<Series[]>([]);
	let events = $state<StateEvent[]>([]);
	/** Set when the hub has no database: there is no history to show. */
	let noHistory = $state(false);
	let range = $state('6h');

	const RANGES = [
		['1h', '1 hour'],
		['6h', '6 hours'],
		['24h', '24 hours'],
		['7d', '7 days']
	] as const;

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

<main class="page">
	{#if missing}
		<p class="notice">Service not found: {host}/{name}</p>
	{:else if !detail}
		<p class="notice">Loading…</p>
	{:else}
		<p class="crumbs legend"><a href="/hosts">Hosts</a> / <a href={hostHref(host)}>{host}</a></p>

		<header class="head plate" data-state={detail.state ?? 'pending'}>
			<div class="what">
				<h1>{detail.service}</h1>
				<p class="muted"><a href={hostHref(host)}>{host}</a>{detail.passive ? ' · passive' : ''}</p>
			</div>
			<div class="status">
				<span class="legend state">{stateName(detail.state)}</span>
				<span class="data muted">
					{#if detail.state === null}
						no result yet
					{:else if detail.state_type === 'soft'}
						SOFT {detail.attempt}/{detail.max_attempts}
					{:else}
						HARD · {age(detail.last_change, live.now)}
					{/if}
				</span>
			</div>
		</header>

		{#if detail.output || detail.long_output}
			<pre class="report plate data">{detail.output}{detail.long_output ? `\n\n${detail.long_output}` : ''}</pre>
		{/if}

		<dl class="facts">
			<dt class="legend">Last check</dt>
			<dd>
				{#if detail.last_check}
					<span class="data">{timestamp(detail.last_check)}</span>
					<span class="muted">({age(detail.last_check, live.now)} ago)</span>
				{:else}–{/if}
			</dd>
			{#if detail.duration_ms !== null}
				<dt class="legend">Execution time</dt>
				<dd class="data">{formatValue(detail.duration_ms / 1000, 's')}</dd>
			{/if}
			<dt class="legend">Last state change</dt>
			<dd class="data">{timestamp(detail.last_hard_change)}</dd>
			{#if detail.check.command}
				<dt class="legend">Interval</dt>
				<dd class="data">
					{duration(detail.check.interval_s)} · retry {duration(detail.check.retry_interval_s)} · timeout {duration(detail.check.timeout_s)}
				</dd>
				<dt class="legend">Command</dt>
				<dd class="data">{detail.check.command}</dd>
			{:else}
				<dt class="legend">Check type</dt>
				<dd class="data">
					passive{#if detail.check.freshness_s !== null} · freshness {duration(detail.check.freshness_s)}{/if}
				</dd>
			{/if}
			<dt class="legend">Source</dt>
			<dd class="data">{detail.check.origin}</dd>
		</dl>

		{#if detail.metrics.length > 0}
			<h2 class="section-label legend">Metrics</h2>
			<table class="list">
				<thead>
					<tr>
						<th>Metric</th>
						<th>Value</th>
						<th class="hide-narrow">Warning</th>
						<th class="hide-narrow">Critical</th>
						<th>State</th>
					</tr>
				</thead>
				<tbody>
					{#each detail.metrics as m (seriesName(m.name, m.labels))}
						<tr>
							<td class="data">{seriesName(m.name, m.labels)}</td>
							<td class="data">{formatValue(m.value, m.unit)}</td>
							<td class="hide-narrow data">{formatThreshold(m.warn, m.unit)}</td>
							<td class="hide-narrow data">{formatThreshold(m.crit, m.unit)}</td>
							<td>{#if m.state}<Lamp state={m.state} />{/if}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		{/if}

		<div class="section-label legend graphs-head">
			<h2>Graphs</h2>
			{#if !noHistory && series.length > 0}
				<div class="ranges" role="group" aria-label="Time range">
					{#each RANGES as [value, label] (value)}
						<button type="button" class="legend" aria-pressed={range === value} onclick={() => (range = value)} title={label}>{value}</button>
					{/each}
				</div>
			{/if}
		</div>
		{#if noHistory}
			<p class="notice">No database configured.</p>
		{:else if series.length === 0}
			<p class="notice">No metrics.</p>
		{:else}
			<div class="graphs">
				{#each series as s (s.id)}
					<Chart series={s} {range} tick={Date.parse(lastCheck ?? '') || 0} />
				{/each}
			</div>
		{/if}

		{#if !noHistory}
			<h2 class="section-label legend">State history</h2>
			{#if events.length === 0}
				<p class="notice">No state changes.</p>
			{:else}
				<table class="list">
					<thead>
						<tr>
							<th>Time</th>
							<th>Change</th>
							<th>Output</th>
						</tr>
					</thead>
					<tbody>
						{#each events as e (e.ts)}
							<tr class:soft={!e.hard}>
								<td class="data nowrap">{timestamp(e.ts)}</td>
								<td class="nowrap">
									{#if e.from_state}<Lamp state={e.from_state} label={false} />{/if}
									<span class="arrow">→</span>
									<Lamp state={e.to_state} stateType={e.state_type} />
								</td>
								<td class="output">{e.output}</td>
							</tr>
						{/each}
					</tbody>
				</table>
			{/if}
		{/if}
	{/if}
</main>

<style>
	.crumbs {
		margin: 0 0 10px;
		font-size: 12px;
		color: var(--ink-2);
	}

	.head {
		display: flex;
		justify-content: space-between;
		align-items: stretch;
		overflow: hidden;
		--c: var(--pending);
	}

	.head[data-state='critical'] {
		--c: var(--crit);
		--on: var(--on-crit);
	}
	.head[data-state='warning'] {
		--c: var(--warn);
		--on: var(--on-warn);
	}
	.head[data-state='unknown'] {
		--c: var(--unknown);
		--on: var(--on-unknown);
	}
	.head[data-state='ok'] {
		--c: var(--ok);
		--on: #f2fff6;
	}

	.what {
		padding: 18px 20px;
		min-width: 0;
	}

	h1 {
		font-size: 26px;
		overflow-wrap: anywhere;
	}

	.what p {
		margin: 4px 0 0;
		font-size: 14px;
	}

	/* The state, as a lit window on the right of the plate. */
	.status {
		display: flex;
		flex-direction: column;
		justify-content: center;
		align-items: flex-end;
		gap: 2px;
		padding: 14px 20px;
		min-width: 190px;
		background: var(--c);
		color: var(--on, var(--ink));
		text-align: right;
	}

	.status .muted {
		color: inherit;
		opacity: 0.85;
		font-size: 12px;
	}

	.head[data-state='pending'] .status {
		background: transparent;
		border-left: 1px dashed var(--rule);
		color: var(--ink-2);
	}

	.state {
		font-size: 22px;
		letter-spacing: 0.12em;
	}

	.report {
		margin: 14px 0 0;
		padding: 14px 16px;
		font-size: 13px;
		line-height: 1.55;
		white-space: pre-wrap;
		overflow-wrap: anywhere;
		background: var(--plate-sunk);
	}

	.facts {
		display: grid;
		grid-template-columns: max-content 1fr;
		gap: 6px 18px;
		margin: 18px 0 0;
		font-size: 14px;
	}

	dt {
		color: var(--ink-2);
		font-size: 12px;
		padding-top: 2px;
	}

	dd {
		margin: 0;
		overflow-wrap: anywhere;
	}

	.graphs-head h2 {
		font: inherit;
	}

	.ranges {
		display: inline-flex;
		order: 2;
		gap: 4px;
	}

	.ranges button {
		font-size: 11px;
		padding: 3px 8px;
		color: var(--ink-2);
		background: var(--plate);
		border: 1px solid var(--rule);
		border-radius: 2px;
		cursor: pointer;
	}

	.ranges button[aria-pressed='true'] {
		background: var(--ink);
		color: var(--plate);
		border-color: var(--ink);
	}

	.graphs {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(min(100%, 520px), 1fr));
		gap: 14px;
	}

	.nowrap {
		white-space: nowrap;
	}

	.arrow {
		margin: 0 6px;
		color: var(--ink-3);
	}

	tr.soft td {
		opacity: 0.75;
	}

	@media (max-width: 640px) {
		.head {
			flex-direction: column;
		}
		.status {
			align-items: flex-start;
			text-align: left;
			min-width: 0;
		}
	}
</style>
