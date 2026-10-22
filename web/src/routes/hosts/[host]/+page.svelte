<script lang="ts">
	import { page } from '$app/state';
	import { api, ApiError, serviceHref } from '#lib/api.ts';
	import { age } from '#lib/format.ts';
	import { live } from '#lib/live.svelte.ts';
	import Lamp from '#lib/Lamp.svelte';

	let name = $derived(page.params.host ?? '');
	let host = $derived(live.hosts.find((h) => h.name === name));
	let services = $derived(live.services.filter((s) => s.host === name));

	let origin = $state<string | null>(null);
	let missing = $state(false);

	$effect(() => {
		const current = name;
		missing = false;
		api.host(current).then(
			(r) => (origin = r.origin),
			(e) => {
				if (e instanceof ApiError && e.status === 404) missing = true;
			}
		);
	});
</script>

<main class="page">
	{#if missing}
		<p class="notice">Host not found: {name}</p>
	{:else if !host}
		<p class="notice">Loading…</p>
	{:else}
		<p class="crumbs legend"><a href="/hosts">Hosts</a></p>
		<header class="plate head">
			<div>
				<h1>{host.name}</h1>
				<p class="data muted">{host.address}{host.node ? ` · node ${host.node}` : ''}</p>
			</div>
			<Lamp state={host.state} />
		</header>

		<dl class="facts">
			{#if host.tags.length > 0}
				<dt class="legend">Tags</dt>
				<dd>{#each host.tags as tag (tag)}<span class="tag data">{tag}</span>{/each}</dd>
			{/if}
			{#each Object.entries(host.vars) as [key, value] (key)}
				<dt class="legend">{key}</dt>
				<dd class="data">{value}</dd>
			{/each}
			{#if origin}
				<dt class="legend">Source</dt>
				<dd class="data">{origin}</dd>
			{/if}
		</dl>

		<h2 class="section-label legend">Services</h2>
		{#if services.length === 0}
			<p class="notice">No services.</p>
		{:else}
			<table class="list">
				<thead>
					<tr>
						<th>State</th>
						<th>Service</th>
						<th class="hide-narrow">Duration</th>
						<th class="hide-narrow">Last check</th>
						<th>Output</th>
					</tr>
				</thead>
				<tbody>
					{#each services as s (s.service)}
						<tr>
							<td><Lamp state={s.state} stateType={s.state_type} /></td>
							<td><a href={serviceHref(s.host, s.service)}>{s.service}</a></td>
							<td class="hide-narrow data">{age(s.last_change, live.now)}</td>
							<td class="hide-narrow data">{s.passive ? 'passive' : age(s.last_check, live.now)}</td>
							<td class="output">{s.output}</td>
						</tr>
					{/each}
				</tbody>
			</table>
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
		align-items: center;
		gap: 16px;
		padding: 18px 20px;
	}

	h1 {
		font-size: 26px;
		overflow-wrap: anywhere;
	}

	.head p {
		margin: 4px 0 0;
		font-size: 13px;
	}

	.facts {
		display: grid;
		grid-template-columns: max-content 1fr;
		gap: 6px 18px;
		margin: 16px 0 0;
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

	.tag {
		display: inline-block;
		margin: 0 6px 4px 0;
		padding: 0 7px;
		font-size: 12px;
		border: 1px solid var(--rule);
		border-radius: 2px;
		background: var(--plate);
	}
</style>
