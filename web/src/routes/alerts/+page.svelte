<!-- Alert history of all hosts and services, like Nagios' Alert History. -->
<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api, ApiError, HISTORY_DELAY, type StateEvent } from '#lib/api.ts';
	import AlertTable from '#lib/AlertTable.svelte';
	import { live } from '#lib/live.svelte.ts';

	const PAGE = 100;
	const TYPES = [
		['', 'All'],
		['host', 'Host'],
		['service', 'Service'],
		['flapping', 'Flapping']
	] as const;
	const STATES = [
		['', 'All'],
		['down', 'DOWN'],
		['up', 'UP'],
		['critical', 'CRITICAL'],
		['warning', 'WARNING'],
		['unknown', 'UNKNOWN'],
		['ok', 'OK'],
		['down,critical,warning,unknown', 'Problems']
	] as const;
	const STATE_TYPES = [
		['', 'All'],
		['hard', 'HARD'],
		['soft', 'SOFT']
	] as const;
	const RANGES = [
		['1h', '1h'],
		['24h', '24h'],
		['7d', '7d'],
		['30d', '30d'],
		['', 'All']
	] as const;

	let params = $derived(page.url.searchParams);
	let filter = $derived({
		host: params.get('host') ?? '',
		service: params.get('service') ?? '',
		type: params.get('type') ?? '',
		state: params.get('state') ?? '',
		state_type: params.get('state_type') ?? '',
		since: params.get('since') ?? '24h'
	});

	let events = $state<StateEvent[]>([]);
	let loading = $state(true);
	/** More (older) events may exist. */
	let more = $state(false);
	let noHistory = $state(false);
	let failed = $state<string | null>(null);

	function set(key: string, value: string) {
		const next = new URLSearchParams(page.url.search);
		if (value === '' && key !== 'since') next.delete(key);
		else next.set(key, value);
		if (key === 'host') next.delete('service');
		goto(`?${next}`, { replace: true, reset: false });
	}

	function query(before?: string) {
		return api.events({
			host: filter.host,
			service: filter.service,
			type: filter.type,
			state: filter.state,
			state_type: filter.state_type === 'hard' || filter.state_type === 'soft' ? filter.state_type : undefined,
			since: filter.since,
			before,
			limit: PAGE
		});
	}

	function loaded(list: StateEvent[], append: boolean) {
		events = append ? [...events, ...list] : list;
		more = list.length === PAGE;
		failed = null;
		loading = false;
	}

	function error(e: unknown) {
		noHistory = e instanceof ApiError && e.noDatabase;
		failed = e instanceof Error ? e.message : String(e);
		loading = false;
	}

	// Refetch the first page when filters change, and after state changes
	// once they are in the database.
	let changes = $derived.by(() => {
		let latest = '';
		let flapping = 0;
		for (const s of live.services) {
			if (s.last_change && s.last_change > latest) latest = s.last_change;
			if (s.flapping) flapping++;
		}
		return `${latest}|${flapping}`;
	});
	$effect(() => {
		const _ = filter;
		query().then((list) => loaded(list, false), error);
	});
	let seen = '';
	$effect(() => {
		const current = changes;
		if (seen === '' || current === seen) {
			seen = current;
			return;
		}
		seen = current;
		const later = setTimeout(() => query().then((list) => loaded(list, false), error), HISTORY_DELAY);
		return () => clearTimeout(later);
	});

	function older() {
		const last = events.at(-1);
		if (last) query(last.ts).then((list) => loaded(list, true), error);
	}

	let hosts = $derived(live.hosts.map((h) => h.name).sort());
</script>

<div class="content">
	<div class="page-head">
		<h1>Alerts{filter.service ? ` · ${filter.host}/${filter.service}` : filter.host ? ` · ${filter.host}` : ''}</h1>
	</div>

	<div class="filters">
		<label>
			Host
			<select value={filter.host} onchange={(e) => set('host', e.currentTarget.value)}>
				<option value="">All</option>
				{#each hosts as h (h)}<option value={h}>{h}</option>{/each}
				{#if filter.host && !hosts.includes(filter.host)}<option value={filter.host}>{filter.host}</option>{/if}
			</select>
		</label>
		{#if filter.service}
			<label>
				Service
				<span class="tag">{filter.service} <button type="button" aria-label="Clear service" onclick={() => set('service', '')}>×</button></span>
			</label>
		{/if}
		<label>
			Alert
			<select value={filter.type} onchange={(e) => set('type', e.currentTarget.value)}>
				{#each TYPES as [v, l] (v)}<option value={v}>{l}</option>{/each}
			</select>
		</label>
		<label>
			Status
			<select value={filter.state} onchange={(e) => set('state', e.currentTarget.value)}>
				{#each STATES as [v, l] (v)}<option value={v}>{l}</option>{/each}
			</select>
		</label>
		<label>
			State Type
			<select value={filter.state_type} onchange={(e) => set('state_type', e.currentTarget.value)}>
				{#each STATE_TYPES as [v, l] (v)}<option value={v}>{l}</option>{/each}
			</select>
		</label>
		<span class="ranges" role="group" aria-label="Time range">
			{#each RANGES as [v, l] (v)}
				<button type="button" aria-pressed={filter.since === v} onclick={() => set('since', v)}>{l}</button>
			{/each}
		</span>
	</div>

	{#if noHistory}
		<p class="notice">No database configured.</p>
	{:else if failed}
		<p class="notice error">{failed}</p>
	{:else if loading}
		<p class="notice">Loading…</p>
	{:else}
		<section class="panel">
			<div class="panel-title">Alert History <span class="muted">{events.length}{more ? '+' : ''}</span></div>
			<AlertTable {events} showKind showHost showService />
			{#if more}
				<div class="older"><button type="button" class="btn small" onclick={older}>Older</button></div>
			{/if}
		</section>
	{/if}
</div>

<style>
	.filters {
		display: flex;
		flex-wrap: wrap;
		align-items: end;
		gap: 8px 14px;
		margin-bottom: 12px;
		font-size: 12px;
	}

	.filters label {
		display: flex;
		flex-direction: column;
		gap: 2px;
		color: var(--muted);
		font-weight: 600;
	}

	.filters select {
		font: inherit;
		font-weight: 400;
		color: var(--text);
		background: var(--panel);
		border: 1px solid var(--line);
		border-radius: 3px;
		padding: 3px 6px;
	}

	.filters .tag {
		color: var(--text);
		font-weight: 400;
		padding: 3px 6px;
	}

	.filters .tag button {
		font: inherit;
		border: 0;
		background: none;
		color: var(--muted);
		cursor: pointer;
		padding: 0 0 0 4px;
	}

	.ranges {
		display: inline-flex;
		gap: 2px;
	}

	.ranges button {
		font: inherit;
		font-size: 11px;
		font-weight: 600;
		padding: 4px 8px;
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

	.older {
		padding: 8px 12px;
	}
</style>
