<script lang="ts">
	import { live } from '#lib/live.svelte.ts';
	import HostProblems from '#lib/HostProblems.svelte';
	import ServiceTable from '#lib/ServiceTable.svelte';

	let problems = $derived(live.services.filter((s) => s.state !== null && s.state !== 'ok'));
	let down = $derived(live.hosts.filter((h) => h.state === 'down'));
</script>

<div class="content">
	<div class="page-head"><h1>Problems</h1></div>
	{#if !live.loaded}
		<p class="notice">Loading…</p>
	{:else}
		<section class="panel">
			<div class="panel-title">Host Problems <span class="muted">{down.length}</span></div>
			{#if down.length > 0}<HostProblems hosts={down} />{:else}<div class="empty">No hosts DOWN.</div>{/if}
		</section>

		<section class="panel">
			<div class="panel-title">Service Problems <span class="muted">{problems.length}</span></div>
			<ServiceTable rows={problems} sort="status" empty="No problems." />
		</section>
	{/if}
</div>
