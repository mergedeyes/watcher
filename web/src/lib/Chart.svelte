<!--
	A metric over time: the average per bucket as a line, min to max as a
	band. Gaps in the data stay gaps.
-->
<script lang="ts">
	import uPlot from 'uplot';
	import 'uplot/dist/uPlot.min.css';
	import { onDestroy } from 'svelte';
	import { api, type Series } from './api.ts';
	import { formatValue, seriesName } from './format.ts';

	interface Props {
		series: Series;
		/** Duration back from now, as the API takes it: `1h`, `6h`, `24h`, `7d`. */
		range: string;
		/** Changes when new data may be there. */
		tick: number;
	}

	let { series, range, tick }: Props = $props();

	let box: HTMLDivElement;
	let plot: uPlot | null = null;
	let empty = $state(false);
	let failed = $state<string | null>(null);
	let latest = $state<number | null>(null);
	let readout = $state<{ time: string; avg: number; min: number; max: number } | null>(null);

	const HEIGHT = 170;

	function css(name: string) {
		return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
	}

	/** Columns for uPlot, with nulls where the data has a gap. */
	function columns(points: [number, number, number, number, number][]) {
		const xs: number[] = [];
		const avg: (number | null)[] = [];
		const min: (number | null)[] = [];
		const max: (number | null)[] = [];
		const steps = points.slice(1).map((p, i) => p[0] - points[i]![0]).sort((a, b) => a - b);
		const typical = steps[Math.floor(steps.length / 2)] ?? 0;
		points.forEach(([t, a, lo, hi], i) => {
			const prev = points[i - 1];
			if (prev && typical > 0 && t - prev[0] > 3 * typical) {
				xs.push((prev[0] + t) / 2000);
				avg.push(null);
				min.push(null);
				max.push(null);
			}
			xs.push(t / 1000);
			avg.push(a);
			min.push(lo);
			max.push(hi);
		});
		return [xs, avg, min, max] as uPlot.AlignedData;
	}

	function draw(data: uPlot.AlignedData) {
		plot?.destroy();
		const ink = css('--ink');
		const ink2 = css('--ink-2');
		const rule = css('--rule');
		const unit = series.unit;
		const axis = { stroke: ink2, grid: { stroke: rule, width: 1 }, ticks: { stroke: rule, width: 1 }, font: '11px "IBM Plex Mono", monospace' };
		const opts: uPlot.Options = {
			width: box.clientWidth,
			height: HEIGHT,
			legend: { show: false },
			cursor: { y: false, points: { size: 6 } },
			scales: { x: { time: true } },
			axes: [
				{
					...axis,
					// Labels in the user's locale: times, or dates for long ranges.
					values: (u, splits) => {
						const span = (u.scales.x?.max ?? 0) - (u.scales.x?.min ?? 0);
						return splits.map((s) => {
							const d = new Date(s * 1000);
							return span > 2 * 86400
								? d.toLocaleDateString([], { month: 'short', day: 'numeric' })
								: d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
						});
					}
				},
				{ ...axis, size: 70, values: (_u, vals) => vals.map((v) => formatValue(v, unit)) }
			],
			series: [
				{},
				{ stroke: ink, width: 1.5, spanGaps: false },
				{ stroke: 'transparent', points: { show: false } },
				{ stroke: 'transparent', points: { show: false } }
			],
			bands: [{ series: [3, 2], fill: `color-mix(in srgb, ${ink} 14%, transparent)` }],
			hooks: {
				setCursor: [
					(u) => {
						const i = u.cursor.idx;
						const a = i == null ? null : u.data[1]?.[i];
						readout =
							i == null || a == null
								? null
								: {
										time: new Date((u.data[0]?.[i] ?? 0) * 1000).toLocaleString(),
										avg: a,
										min: u.data[2]?.[i] ?? a,
										max: u.data[3]?.[i] ?? a
									};
					}
				]
			}
		};
		plot = new uPlot(opts, data, box);
	}

	$effect(() => {
		// Re-run when the range or tick changes.
		const _ = [range, tick, series.id];
		const controller = new AbortController();
		api.points(series.id, range, 300, controller.signal).then(
			(p) => {
				failed = null;
				empty = p.points.length === 0;
				latest = p.points.at(-1)?.[1] ?? null;
				if (!empty) draw(columns(p.points));
				else {
					plot?.destroy();
					plot = null;
				}
			},
			(e) => {
				if (e.name !== 'AbortError') failed = e.message;
			}
		);
		return () => controller.abort();
	});

	$effect(() => {
		const observer = new ResizeObserver(() => plot?.setSize({ width: box.clientWidth, height: HEIGHT }));
		observer.observe(box);
		return () => observer.disconnect();
	});

	onDestroy(() => plot?.destroy());
</script>

<figure class="plate chart">
	<figcaption>
		<span class="data name">{seriesName(series.metric, series.labels)}</span>
		<span class="data value">
			{#if readout}
				{readout.time} · {formatValue(readout.avg, series.unit)}
				{#if readout.min !== readout.max}
					<span class="muted">({formatValue(readout.min, series.unit)} – {formatValue(readout.max, series.unit)})</span>
				{/if}
			{:else if latest !== null}
				{formatValue(latest, series.unit)}
			{/if}
		</span>
	</figcaption>
	<div class="canvas" bind:this={box}>
		{#if failed}
			<p class="note">Couldn't load the data: {failed}</p>
		{:else if empty}
			<p class="note">No data in this time range.</p>
		{/if}
	</div>
</figure>

<style>
	.chart {
		margin: 0;
		padding: 12px 14px 8px;
	}

	figcaption {
		display: flex;
		justify-content: space-between;
		gap: 12px;
		flex-wrap: wrap;
		font-size: 13px;
		margin-bottom: 6px;
	}

	.name {
		font-weight: 500;
	}

	.value {
		color: var(--ink-2);
	}

	.canvas {
		position: relative;
		min-height: 170px;
	}

	.note {
		position: absolute;
		inset: 0;
		display: grid;
		place-items: center;
		margin: 0;
		color: var(--ink-3);
		font-size: 13px;
	}

	.canvas :global(.u-select) {
		background: color-mix(in srgb, var(--focus) 15%, transparent);
	}
</style>
