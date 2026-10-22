<script lang="ts">
	import { hostHref, type HostView } from '#lib/api.ts';
	import { live } from '#lib/live.svelte.ts';
	import Window from '#lib/Window.svelte';

	let query = $state('');

	// Windows keep their place (config order), like on a real panel, so a
	// lit window is recognised by where it is.
	let shown = $derived.by(() => {
		const q = query.trim().toLowerCase();
		if (!q) return live.hosts;
		return live.hosts.filter(
			(h) =>
				h.name.toLowerCase().includes(q) ||
				h.address.toLowerCase().includes(q) ||
				h.tags.some((t) => t.toLowerCase().includes(q))
		);
	});

	function detail(h: HostView) {
		const c = h.services;
		const total = c.ok + c.warning + c.critical + c.unknown + c.pending;
		const bad = c.critical + c.warning + c.unknown;
		if (!h.enabled) return 'disabled';
		if (total === 0) return 'no services';
		if (bad > 0) return `${bad} of ${total} tripped`;
		return total === 1 ? '1 service' : `${total} services`;
	}
</script>

<main class="page">
	<div class="head">
		<h1 class="legend">Hosts</h1>
		<input type="search" placeholder="Filter by name, address or tag" bind:value={query} aria-label="Filter hosts" />
	</div>

	{#if !live.loaded}
		<p class="notice">Loading…</p>
	{:else if live.hosts.length === 0}
		<p class="notice">No hosts are configured yet. Add <code>[[host]]</code> entries to your configuration.</p>
	{:else if shown.length === 0}
		<p class="notice">No host matches “{query}”.</p>
	{:else}
		<div class="panel">
			{#each shown as h (h.name)}
				<Window
					href={hostHref(h.name)}
					title={h.name}
					caption={h.address !== h.name ? h.address : undefined}
					state={h.state}
					now={live.now}
					detail={detail(h)}
				/>
			{/each}
		</div>
	{/if}
</main>

<style>
	.head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 16px;
		flex-wrap: wrap;
		margin-bottom: 20px;
	}

	h1 {
		font-size: 20px;
	}

	.panel {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(170px, 1fr));
		gap: 14px;
		padding: 18px;
		background: var(--plate-sunk);
		border: 1px solid var(--rule);
		border-radius: var(--radius);
		box-shadow: inset 0 2px 6px rgb(0 0 0 / 0.08);
	}

	@media (max-width: 640px) {
		.panel {
			grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
			padding: 12px;
		}
	}
</style>
