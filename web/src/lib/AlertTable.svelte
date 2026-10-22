<!-- Alert history: service and host state changes and flapping, newest first. -->
<script lang="ts">
	import { hostHref, serviceHref, type StateEvent } from './api.ts';
	import { datetime } from './format.ts';
	import Status from './Status.svelte';

	interface Props {
		events: StateEvent[];
		showHost?: boolean;
		showService?: boolean;
		/** The Alert column: HOST, SERVICE or FLAPPING. */
		showKind?: boolean;
		empty?: string;
	}

	let { events, showHost = false, showService = false, showKind = false, empty = 'No alerts.' }: Props = $props();

	const KIND = { service: 'SERVICE', host: 'HOST', flap_start: 'FLAPPING', flap_stop: 'FLAPPING' } as const;
</script>

{#if events.length === 0}
	<div class="empty">{empty}</div>
{:else}
	<table class="grid">
		<thead>
			<tr>
				<th>Time</th>
				{#if showKind}<th>Alert</th>{/if}
				{#if showHost}<th>Host</th>{/if}
				{#if showService}<th>Service</th>{/if}
				<th>Status</th>
				<th class="hide-narrow">State Type</th>
				<th>Status Information</th>
			</tr>
		</thead>
		<tbody>
			{#each events as e, i (i)}
				<tr>
					<td class="nowrap">{datetime(e.ts)}</td>
					{#if showKind}<td class="nowrap">{KIND[e.kind]}</td>{/if}
					{#if showHost}<td class="nowrap"><a href={hostHref(e.host)}>{e.host}</a></td>{/if}
					{#if showService}
						<td class="nowrap">
							{#if e.kind === 'host'}<span class="muted">{e.service}</span>{:else}<a href={serviceHref(e.host, e.service)}>{e.service}</a>{/if}
						</td>
					{/if}
					<td class="nowrap">
						{#if e.kind === 'flap_start'}
							<span class="tag flap">STARTED</span>
						{:else if e.kind === 'flap_stop'}
							<span class="tag">STOPPED</span>
						{:else}
							<Status state={e.to_state} />
						{/if}
					</td>
					<td class="hide-narrow">{e.kind === 'service' || e.kind === 'host' ? e.state_type.toUpperCase() : ''}</td>
					<td class="info">{e.output}</td>
				</tr>
			{/each}
		</tbody>
	</table>
{/if}

<style>
	.tag.flap {
		margin-left: 0;
	}
</style>
