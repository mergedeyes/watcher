<!-- Hosts that are DOWN, as in the Nagios host problems table. -->
<script lang="ts">
	import { hostHref, type HostView } from './api.ts';
	import { datetime, since } from './format.ts';
	import { live } from './live.svelte.ts';
	import Status from './Status.svelte';

	let { hosts }: { hosts: HostView[] } = $props();
</script>

<table class="grid">
	<thead>
		<tr>
			<th>Host</th>
			<th>Status</th>
			<th class="hide-narrow">Last Check</th>
			<th class="hide-narrow">Duration</th>
			<th class="hide-narrow">Attempt</th>
			<th>Status Information</th>
		</tr>
	</thead>
	<tbody>
		{#each hosts as h (h.name)}
			<tr data-host={h.state}>
				<td class="nowrap"><a href={hostHref(h.name)}>{h.name}</a></td>
				<td class="nowrap">
					<Status state={h.state} />
					{#if h.flapping}<span class="tag flap">FLAPPING</span>{/if}
				</td>
				<td class="nowrap hide-narrow">{datetime(h.last_check)}</td>
				<td class="nowrap hide-narrow">{since(h.last_change, live.now)}</td>
				<td class="nowrap hide-narrow">
					{#if h.attempt !== null}{h.attempt}/{h.max_attempts}{#if h.state_type === 'soft'}&nbsp;<span class="tag">SOFT</span>{/if}{/if}
				</td>
				<td class="info">{h.output}</td>
			</tr>
		{/each}
	</tbody>
</table>
