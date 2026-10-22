<!-- Status totals: one box per state, coloured when a problem count is non-zero. -->
<script lang="ts">
	export interface Total {
		key: string;
		label: string;
		count: number;
		href: string;
		/** Colour when non-zero; omit for neutral boxes (totals, OK). */
		state?: 'ok' | 'warning' | 'critical' | 'unknown' | 'pending';
	}

	let { title, items }: { title: string; items: Total[] } = $props();
</script>

<div class="totals panel">
	<div class="panel-title">{title}</div>
	<div class="boxes">
		{#each items as t (t.key)}
			<a class="box" href={t.href} data-state={t.count > 0 ? t.state : undefined}>
				<span class="count">{t.count}</span>
				<span class="label">{t.label}</span>
			</a>
		{/each}
	</div>
</div>

<style>
	.boxes {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(96px, 1fr));
	}

	.box {
		display: flex;
		flex-direction: column;
		align-items: center;
		padding: 10px 6px;
		color: var(--text);
		border-right: 1px solid var(--line);
		text-decoration: none;
	}

	.box:last-child {
		border-right: 0;
	}

	.box:hover {
		background: var(--panel-alt);
		text-decoration: none;
	}

	.count {
		font-size: 22px;
		font-weight: 600;
		line-height: 1.2;
		font-variant-numeric: tabular-nums;
	}

	.label {
		font-size: 11px;
		font-weight: 600;
		color: var(--muted);
		text-transform: uppercase;
		letter-spacing: 0.03em;
	}

	.box[data-state='ok'] .count {
		color: var(--ok);
	}
	.box[data-state='warning'] {
		background: var(--warn);
	}
	.box[data-state='critical'] {
		background: var(--crit);
	}
	.box[data-state='unknown'] {
		background: var(--unknown);
	}
	.box[data-state='warning'] .count,
	.box[data-state='warning'] .label {
		color: var(--on-warn);
	}
	.box[data-state='critical'] .count,
	.box[data-state='critical'] .label,
	.box[data-state='unknown'] .count,
	.box[data-state='unknown'] .label {
		color: #fff;
	}
	.box[data-state='warning']:hover,
	.box[data-state='critical']:hover,
	.box[data-state='unknown']:hover {
		filter: brightness(1.05);
	}
</style>
