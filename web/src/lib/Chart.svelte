<!--
	A metric over time: the average per bucket as a line, min to max as a
	band. Gaps in the data stay gaps. A zoomed section is loaded again, in
	finer buckets. Warning and critical thresholds are
	dashed lines, and the y-axis always shows them.
-->
<script lang="ts">
	import uPlot from 'uplot';
	import 'uplot/dist/uPlot.min.css';
	import { onDestroy } from 'svelte';
	import { api, type Series } from './api.ts';
	import { formatTicks, formatValue, seriesName, tickSteps } from './format.ts';

	interface Props {
		series: Series;
		/** Duration back from now, as the API takes it: `30s`, `1h`, `7d`. */
		range: string;
		/** The same range in seconds; the time axis always spans all of it. */
		rangeSecs: number;
		/** Changes when new data may be there. */
		tick: number;
	}

	let { series, range, rangeSecs, tick }: Props = $props();

	let box: HTMLDivElement;
	let plot: uPlot | null = null;
	let empty = $state(false);
	let failed = $state<string | null>(null);
	let latest = $state<number | null>(null);
	let readout = $state<{ time: string; avg: number; min: number; max: number } | null>(null);
	/** A section selected by dragging (unix seconds); null shows the whole range. */
	let zoom = $state<{ min: number; max: number } | null>(null);
	/** The whole range of the current drawing and its data, for resetting the zoom. */
	let full = { from: 0, to: 0 };
	let fullData: uPlot.AlignedData | null = null;
	let drawnRange = '';
	/** The mouse button is down in the graph: a selection is being dragged. */
	let dragging = false;
	/** Drawing of data that arrived during a drag, done when the drag ends. */
	let pending: (() => void) | null = null;

	const HEIGHT = 170;
	const FONT = '11px ui-monospace, Menlo, Consolas, monospace';

	let measurer: CanvasRenderingContext2D | null = null;

	/** Width of the y-axis: the widest label plus ticks and gap. */
	function axisSize(labels: string[] | null) {
		measurer ??= document.createElement('canvas').getContext('2d');
		if (!measurer) return 70;
		measurer.font = FONT;
		const widest = Math.max(0, ...(labels ?? []).map((l) => measurer!.measureText(l).width));
		return Math.ceil(widest) + 16;
	}

	function css(name: string) {
		return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
	}

	/** Dashed lines across the plot at the threshold bounds. */
	function thresholds(u: uPlot) {
		const { ctx, bbox } = u;
		ctx.save();
		ctx.lineWidth = 1.5 * devicePixelRatio;
		ctx.setLineDash([6 * devicePixelRatio, 4 * devicePixelRatio]);
		for (const [bounds, color] of [
			[series.warn, css('--warn')],
			[series.crit, css('--crit')]
		] as const) {
			ctx.strokeStyle = color;
			for (const v of bounds) {
				const y = Math.round(u.valToPos(v, 'y', true));
				if (y < bbox.top || y > bbox.top + bbox.height) continue;
				ctx.beginPath();
				ctx.moveTo(bbox.left, y);
				ctx.lineTo(bbox.left + bbox.width, y);
				ctx.stroke();
			}
		}
		ctx.restore();
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

	/**
	 * Draws `data` on a time axis from `from` to `to` (unix seconds). An
	 * existing graph only gets new data, so a zoom stays; during a drag the
	 * update waits, or the selection would be lost.
	 */
	function draw(data: uPlot.AlignedData, from: number, to: number) {
		if (dragging) {
			pending = () => draw(data, from, to);
			return;
		}
		full = { from, to };
		fullData = data;
		if (plot) {
			// A zoomed section has its own data.
			if (!zoom) plot.setData(data);
			return;
		}
		const ink = css('--link');
		const ink2 = css('--muted');
		const rule = css('--line');
		const unit = series.unit;
		const axis = { stroke: ink2, grid: { stroke: rule, width: 1 }, ticks: { stroke: rule, width: 1 }, font: FONT };
		const opts: uPlot.Options = {
			width: box.clientWidth,
			height: HEIGHT,
			legend: { show: false },
			cursor: {
				y: false,
				points: { size: 6 },
				// Double-click resets the zoom, like the button.
				bind: { dblclick: () => () => (resetZoom(), null) }
			},
			scales: {
				x: { time: true, range: () => (zoom ? [zoom.min, zoom.max] : [full.from, full.to]) },
				// The data and the thresholds, padded like uPlot's default.
				y: {
					range: (_u, min, max) => {
						const all = [min, max, ...series.warn, ...series.crit].filter((v) => v != null && isFinite(v));
						if (all.length === 0) return [0, 1];
						return uPlot.rangeNum(Math.min(...all), Math.max(...all), 0.1, true);
					}
				}
			},
			axes: [
				{
					...axis,
					// Labels in the user's locale: times, or dates for long ranges.
					values: (u, splits) => {
						const span = (u.scales.x?.max ?? 0) - (u.scales.x?.min ?? 0);
						return splits.map((s) => {
							const d = new Date(s * 1000);
							if (span > 2 * 86400) return d.toLocaleDateString([], { month: 'short', day: 'numeric' });
							// Short spans (and zoomed sections) need seconds to tell ticks apart.
							const seconds = span <= 600 ? { second: '2-digit' as const } : {};
							return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', ...seconds });
						});
					}
				},
				{ ...axis, incrs: tickSteps(unit), size: (_u, labels) => axisSize(labels), values: (_u, vals) => formatTicks(vals, unit) }
			],
			series: [
				{},
				{ stroke: ink, width: 1.5, spanGaps: false },
				{ stroke: 'transparent', points: { show: false } },
				{ stroke: 'transparent', points: { show: false } }
			],
			bands: [{ series: [3, 2], fill: `color-mix(in srgb, ${ink} 18%, transparent)` }],
			hooks: {
				draw: [thresholds],
				init: [(u) => u.over.addEventListener('mousedown', () => (dragging = true))],
				// Dragging selects a section; uPlot then sets the scale to it.
				setSelect: [
					(u) => {
						if (u.select.width < 2) return;
						zoom = {
							min: u.posToVal(u.select.left, 'x'),
							max: u.posToVal(u.select.left + u.select.width, 'x')
						};
					}
				],
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

	// A drag ends anywhere on the page. Draw held-back data after uPlot has
	// handled the release (and applied the selection).
	$effect(() => {
		const release = () => {
			if (!dragging) return;
			dragging = false;
			setTimeout(() => {
				if (pending && !dragging) {
					const run = pending;
					pending = null;
					run();
				}
			});
		};
		window.addEventListener('mouseup', release);
		return () => window.removeEventListener('mouseup', release);
	});

	function resetZoom() {
		zoom = null;
		if (fullData) plot?.setData(fullData);
		plot?.setScale('x', { min: full.from, max: full.to });
	}

	// A zoomed section, loaded again: the whole range has too few buckets
	// to show its detail.
	$effect(() => {
		void tick;
		const section = zoom;
		if (!section) return;
		const controller = new AbortController();
		const [from, to] = [String(Math.floor(section.min)), String(Math.ceil(section.max))];
		api.points(series.id, from, 300, controller.signal, to).then(
			(p) => {
				const show = () => {
					if (zoom === section) plot?.setData(columns(p.points));
				};
				if (dragging) pending = show;
				else show();
			},
			(e) => {
				if (e.name !== 'AbortError') failed = e.message;
			}
		);
		return () => controller.abort();
	});

	$effect(() => {
		// Re-run when the range or tick changes.
		const _ = [range, tick, series.id];
		const controller = new AbortController();
		const to = Date.now() / 1000;
		const from = to - rangeSecs;
		// A new range starts unzoomed; new data for the same range keeps the zoom.
		if (range !== drawnRange) {
			zoom = null;
			drawnRange = range;
		}
		api.points(series.id, range, 300, controller.signal).then(
			(p) => {
				failed = null;
				empty = p.points.length === 0;
				latest = p.points.at(-1)?.[1] ?? null;
				if (!empty) draw(columns(p.points), from, to);
				else {
					plot?.destroy();
					plot = null;
					pending = null;
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

<figure class="panel chart">
	<figcaption>
		<span class="mono name">{seriesName(series.metric, series.labels)}</span>
		<span class="mono value">
			{#if readout}
				{readout.time} · {formatValue(readout.avg, series.unit)}
				{#if readout.min !== readout.max}
					<span class="muted">({formatValue(readout.min, series.unit)} – {formatValue(readout.max, series.unit)})</span>
				{/if}
			{:else if latest !== null}
				{formatValue(latest, series.unit)}
			{/if}
			{#if zoom}
				<button type="button" class="reset btn" onclick={resetZoom}>Reset zoom</button>
			{/if}
		</span>
	</figcaption>
	<div class="canvas" bind:this={box}>
		{#if failed}
			<p class="note">Error: {failed}</p>
		{:else if empty}
			<p class="note">No data.</p>
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

	.reset {
		margin-left: 10px;
		font-size: 11px;
		padding: 2px 8px;
		color: var(--text);
		background: var(--panel);
		border: 1px solid var(--line);
		border-radius: 2px;
		cursor: pointer;
	}

	.value {
		color: var(--muted);
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
		color: var(--muted);
		font-size: 13px;
	}

	.canvas :global(.u-select) {
		background: color-mix(in srgb, var(--focus) 15%, transparent);
	}
</style>
