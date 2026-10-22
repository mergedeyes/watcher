<script lang="ts">
	import { page } from '$app/state';
	import { hostHref, serviceHref } from '#lib/api.ts';
	import { age, stateName } from '#lib/format.ts';
	import { live } from '#lib/live.svelte.ts';
	import Lamp from '#lib/Lamp.svelte';

	const STATES = ['critical', 'warning', 'unknown', 'pending', 'ok'] as const;
	type Filter = (typeof STATES)[number];

	let filter = $derived(
		(STATES as readonly string[]).includes(page.url.searchParams.get('state') ?? '')
			? (page.url.searchParams.get('state') as Filter)
			: null
	);
	let query = $state('');

	let shown = $derived.by(() => {
		const q = query.trim().toLowerCase();
		return live.services.filter(
			(s) =>
				(filter === null || (s.state ?? 'pending') === filter) &&
				(!q || `${s.host}/${s.service} ${s.output}`.toLowerCase().includes(q))
		);
	});
</script>

<main class="page">
	<div class="head">
		<h1 class="legend">Services</h1>
		<input type="search" placeholder="Filter (name, output)" bind:value={query} aria-label="Filter services" />
	</div>

	<nav class="chips" aria-label="Filter by state">
		<a href="/services" class="chip legend" aria-current={filter === null ? 'true' : undefined}>All</a>
		{#each STATES as s (s)}
			<a href={`/services?state=${s}`} class="chip legend" aria-current={filter === s ? 'true' : undefined}>
				<Lamp state={s === 'pending' ? null : s} label={false} />
				{stateName(s === 'pending' ? null : s)}
			</a>
		{/each}
	</nav>

	{#if !live.loaded}
		<p class="notice">Loading…</p>
	{:else if shown.length === 0}
		<p class="notice">
			{#if live.services.length === 0}
				No services configured.
			{:else}
				No matches. <a href="/services">Show all</a>
			{/if}
		</p>
	{:else}
		<table class="list">
			<thead>
				<tr>
					<th>State</th>
					<th>Host</th>
					<th>Service</th>
					<th class="hide-narrow">Duration</th>
					<th>Output</th>
				</tr>
			</thead>
			<tbody>
				{#each shown as s (`${s.host}/${s.service}`)}
					<tr>
						<td><Lamp state={s.state} stateType={s.state_type} /></td>
						<td><a href={hostHref(s.host)}>{s.host}</a></td>
						<td><a href={serviceHref(s.host, s.service)}>{s.service}</a></td>
						<td class="hide-narrow data">{age(s.last_change, live.now)}</td>
						<td class="output">{s.output}</td>
					</tr>
				{/each}
			</tbody>
		</table>
		<p class="muted count data">{shown.length}/{live.services.length}</p>
	{/if}
</main>

<style>
	.head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 16px;
		flex-wrap: wrap;
		margin-bottom: 14px;
	}

	h1 {
		font-size: 20px;
	}

	.chips {
		display: flex;
		flex-wrap: wrap;
		gap: 8px;
		margin-bottom: 16px;
	}

	.chip {
		display: inline-flex;
		align-items: center;
		gap: 7px;
		padding: 5px 11px;
		font-size: 12px;
		text-decoration: none;
		background: var(--plate);
		border: 1px solid var(--rule);
		border-radius: 2px;
	}

	.chip[aria-current] {
		background: var(--ink);
		color: var(--plate);
		border-color: var(--ink);
	}

	.count {
		font-size: 13px;
		margin-top: 10px;
	}
</style>
