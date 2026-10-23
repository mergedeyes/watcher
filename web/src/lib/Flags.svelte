<!-- Badges for what applies to a host or service: acknowledged, downtime, muted, checks off. -->
<script lang="ts">
	let { v }: { v: { acknowledged: boolean; downtime: boolean; muted: boolean; checks_enabled: boolean } } = $props();
</script>

{#if v.acknowledged}<span class="tag ack" title="Problem acknowledged">ACK</span>{/if}
{#if v.downtime}<span class="tag downtime" title="In a downtime">DOWNTIME</span>{/if}
{#if v.muted}<span class="tag muted" title="Notifications off">MUTED</span>{/if}
{#if !v.checks_enabled}<span class="tag off" title="Active checks disabled">CHECKS OFF</span>{/if}
