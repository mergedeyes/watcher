<script lang="ts">
	import HostProblemsPanel from '#lib/HostProblemsPanel.svelte';
	import ServicePanel from '#lib/ServicePanel.svelte';
</script>

<div class="content">
	<div class="page-head"><h1>Problems</h1></div>
	<HostProblemsPanel title="Host Problems" />
	<ServicePanel title="Service Problems" filter={{ problems: true }} sort="status" empty="No problems." />
</div>
