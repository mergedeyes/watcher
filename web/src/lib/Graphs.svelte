<!-- The graphs panel of a service: one chart per series, with time ranges. -->
<script lang="ts">
	import type { Snippet } from 'svelte';
	import { api, ApiError, type Series } from './api.ts';
	import Chart from './Chart.svelte';
	import { live } from './live.svelte.ts';

	interface Props {
		host: string;
		service: string;
		/** Changes when new data may be there. */
		tick: number;
		/** Extra controls in the panel title, e.g. a service picker. */
		controls?: Snippet;
	}

	let { host, service, tick, controls }: Props = $props();

	let series = $state<Series[]>([]);
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

	$effect(() => {
		const [h, s] = [host, service];
		api.series(h, s).then(
			(list) => (series = list),
			(e) => (noHistory = e instanceof ApiError && e.noDatabase)
		);
	});
</script>

<section class="panel">
	<div class="panel-title">
		Graphs
		{@render controls?.()}
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
				<Chart series={s} range={range[0]} rangeSecs={range[1]} {tick} />
			{/each}
		</div>
	{/if}
</section>

<style>
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
