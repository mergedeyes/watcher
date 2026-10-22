<script lang="ts">
	import { live } from '#lib/live.svelte.ts';
	import ServiceTable from '#lib/ServiceTable.svelte';

	let problems = $derived(live.services.filter((s) => s.state !== null && s.state !== 'ok'));
</script>

<div class="content">
	<div class="page-head"><h1>Service Problems</h1></div>
	{#if !live.loaded}
		<p class="notice">Loading…</p>
	{:else}
		<section class="panel">
			<div class="panel-title">Unhandled <span class="muted">{problems.length}</span></div>
			<ServiceTable rows={problems} sort="status" empty="No problems." />
		</section>
	{/if}
</div>
