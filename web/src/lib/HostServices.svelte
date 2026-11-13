<script lang="ts">
	import { untrack } from 'svelte';
	import Combobox from '#lib/Combobox.svelte';
	import { SERVICE_ROW_KEYS, type ServiceRow } from '#lib/schema.ts';
	import type { Suggest } from '#lib/suggest.ts';

	interface Props {
		rows: ServiceRow[];
		/** Where each row comes from: `servicegroup linux`, `host check`, ... */
		from: (row: ServiceRow) => string;
		/** The interval that applies when the row sets none. */
		interval: (name: string) => string;
		/** The entries as written, for keys the form doesn't show. */
		written: Record<string, unknown>;
		suggest: Suggest;
		/** A row to open and scroll to (`?focus=` from a service page). */
		focus: string | null;
	}

	let { rows = $bindable(), from, interval, written, suggest, focus }: Props = $props();

	// Opened once, when the page opens with a focus; later the user decides.
	let open = $state<string[]>(untrack(() => (focus ? [focus] : [])));

	function toggle(name: string) {
		open = open.includes(name) ? open.filter((n) => n !== name) : [...open, name];
	}

	/** How many settings a row has, for the closed row. */
	function count(row: ServiceRow): number {
		return (row.interval.trim() ? 1 : 0) + row.vars.filter(([k]) => k.trim()).length + row.thresholds.filter(([m]) => m.trim()).length;
	}

	/** Keys of the entry the form doesn't show: kept as written. */
	function kept(row: ServiceRow): string[] {
		const entry = written[row.orig];
		if (typeof entry !== 'object' || entry === null || Array.isArray(entry)) return [];
		return Object.keys(entry).filter((k) => !SERVICE_ROW_KEYS.includes(k));
	}

	const DURATIONS = ['30s', '1m', '2m', '5m', '10m', '15m', '30m', '1h'];
</script>

<div class="table">
	{#if rows.length === 0}
		<p class="muted none">No services yet: add some above.</p>
	{:else}
		<div class="row head"><span>Service</span><span>From</span><span>Enabled</span><span></span></div>
	{/if}
	{#each rows as row (row.name)}
		{@const isOpen = open.includes(row.name)}
		<div class="item" class:focused={focus === row.name}>
			<div class="row">
				<button type="button" class="name mono" aria-expanded={isOpen} onclick={() => toggle(row.name)}>
					<span class="caret">{isOpen ? '▾' : '▸'}</span>{row.name}
					{#if count(row) > 0}<span class="set">{count(row)} set</span>{/if}
				</button>
				<span class="from muted">{from(row)}</span>
				<select bind:value={row.enabled} aria-label="Enabled on this host">
					<option value="">—</option>
					<option value="yes">yes</option>
					<option value="no">no</option>
				</select>
				{#if row.own}
					<button type="button" class="btn" aria-label="Remove {row.name}" title="Remove from this host"
						onclick={() => (rows = rows.filter((r) => r !== row))}>×</button>
				{:else}
					<span></span>
				{/if}
			</div>
			{#if isOpen}
				<div class="settings">
					<label for="hs-{row.name}-interval">Check Interval</label>
					<div>
						<Combobox id="hs-{row.name}-interval" mono placeholder={interval(row.name)} bind:value={row.interval} options={DURATIONS} />
					</div>
					<span class="label">Variables</span>
					<div class="rows">
						{#each row.vars as v, i (i)}
							<div class="pair two">
								<Combobox mono placeholder="name" bind:value={v[0]} label="Variable name" options={suggest.varNames()} />
								<Combobox mono placeholder="value" bind:value={v[1]} label="Variable value" options={suggest.varValues(v[0])} />
								<button type="button" class="btn" aria-label="Remove" onclick={() => (row.vars = row.vars.filter((_, j) => j !== i))}>×</button>
							</div>
						{/each}
						<button type="button" class="btn small" onclick={() => (row.vars = [...row.vars, ['', '']])}>Add variable</button>
					</div>
					<span class="label">Thresholds</span>
					<div class="rows">
						{#each row.thresholds as t, i (i)}
							<div class="pair three">
								<Combobox mono placeholder="metric" bind:value={t[0]} label="Metric" options={suggest.metrics()} />
								<Combobox mono placeholder="warn, e.g. > 100" bind:value={t[1]} label="Warning" options={suggest.thresholds()} />
								<Combobox mono placeholder="crit, e.g. > 500" bind:value={t[2]} label="Critical" options={suggest.thresholds()} />
								<button type="button" class="btn" aria-label="Remove" onclick={() => (row.thresholds = row.thresholds.filter((_, j) => j !== i))}>×</button>
							</div>
						{/each}
						<button type="button" class="btn small" onclick={() => (row.thresholds = [...row.thresholds, ['', '', '']])}>Add threshold</button>
					</div>
					{#if kept(row).length > 0}
						<p class="muted kept">Also set in the file, kept as written: <span class="mono">{kept(row).join(', ')}</span></p>
					{/if}
				</div>
			{/if}
		</div>
	{/each}
</div>

<style>
	.table {
		display: grid;
		gap: 2px;
		max-width: 720px;
	}

	.none {
		margin: 0;
		font-size: 12px;
	}

	.row {
		display: grid;
		grid-template-columns: minmax(0, 1.4fr) minmax(0, 1.4fr) 70px 28px;
		gap: 6px;
		align-items: center;
	}

	.row.head {
		font-size: 11px;
		font-weight: 600;
		color: var(--muted);
	}

	.item {
		border-top: 1px solid var(--line);
		padding: 3px 0;
	}

	.item.focused {
		outline: 2px solid var(--link);
		outline-offset: 2px;
		border-radius: 2px;
	}

	.name {
		display: flex;
		gap: 6px;
		align-items: baseline;
		min-width: 0;
		border: 0;
		padding: 2px 0;
		background: none;
		color: inherit;
		font-size: 13px;
		text-align: left;
		cursor: pointer;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.caret {
		width: 10px;
		color: var(--muted);
	}

	.set {
		font-size: 11px;
		color: var(--link);
	}

	.from {
		font-size: 12px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.settings {
		display: grid;
		grid-template-columns: 110px minmax(0, 1fr);
		gap: 6px 10px;
		align-items: start;
		margin: 6px 0 6px 16px;
	}

	.settings label,
	.settings .label {
		font-size: 12px;
		color: var(--muted);
		padding-top: 5px;
	}

	.rows {
		display: grid;
		gap: 4px;
	}

	.pair {
		display: grid;
		gap: 4px;
	}

	.pair.two {
		grid-template-columns: minmax(0, 1fr) minmax(0, 2fr) auto;
	}

	.pair.three {
		grid-template-columns: minmax(0, 1fr) minmax(0, 1fr) minmax(0, 1fr) auto;
	}

	.btn.small {
		justify-self: start;
		font-size: 12px;
		padding: 3px 8px;
	}

	.kept {
		grid-column: 1 / -1;
		margin: 0;
		font-size: 12px;
	}

	@media (max-width: 720px) {
		.settings {
			grid-template-columns: 1fr;
			margin-left: 0;
		}
	}
</style>
