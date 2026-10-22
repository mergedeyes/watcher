<script lang="ts">
	import { page } from '$app/state';
	import { api, ApiError } from '#lib/api.ts';
	import { live } from '#lib/live.svelte.ts';
	import ServiceTable from '#lib/ServiceTable.svelte';
	import Status from '#lib/Status.svelte';

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

<div class="content">
	{#if missing}
		<p class="notice">Host not found: {name}</p>
	{:else if !host}
		<p class="notice">Loading…</p>
	{:else}
		<div class="page-head">
			<h1>{host.name}</h1>
			<Status state={host.state} />
		</div>

		<section class="panel">
			<div class="panel-title">Host Information</div>
			<table class="grid kv">
				<tbody>
					<tr><th>Address</th><td class="mono">{host.address}</td></tr>
					{#if host.node}<tr><th>Node</th><td>{host.node}</td></tr>{/if}
					<tr><th>Tags</th><td>{#each host.tags as t (t)}<span class="tag">{t}</span> {/each}</td></tr>
					{#each Object.entries(host.vars) as [k, v] (k)}
						<tr><th>vars.{k}</th><td class="mono">{v}</td></tr>
					{/each}
					{#if !host.enabled}<tr><th>Enabled</th><td>no</td></tr>{/if}
					{#if origin}<tr><th>Source</th><td class="mono">{origin}</td></tr>{/if}
				</tbody>
			</table>
		</section>

		<section class="panel">
			<div class="panel-title">Services <span class="muted">{services.length}</span></div>
			<ServiceTable rows={services} showHost={false} sort="service" />
		</section>
	{/if}
</div>

<style>
	.kv th {
		width: 160px;
		background: var(--panel-alt);
		vertical-align: top;
		border-bottom: 1px solid var(--line);
	}
</style>
