<!-- The notification log: who got what, over which channel, and whether it arrived. Newest first. -->
<script lang="ts">
	import type { SentNotification } from './api.ts';
	import { datetime } from './format.ts';
	import Status from './Status.svelte';

	interface Props {
		notifications: SentNotification[];
		empty?: string;
	}

	let { notifications, empty = 'No notifications.' }: Props = $props();

	const KIND = {
		problem: 'PROBLEM',
		recovery: 'RECOVERY',
		flappingstart: 'FLAPPINGSTART',
		flappingstop: 'FLAPPINGSTOP',
		downtimestart: 'DOWNTIMESTART',
		downtimeend: 'DOWNTIMEEND'
	} as const;
</script>

{#if notifications.length === 0}
	<div class="empty">{empty}</div>
{:else}
	<table class="grid">
		<thead>
			<tr>
				<th>Time</th>
				<th>Type</th>
				<th>Status</th>
				<th>Contact</th>
				<th class="hide-narrow">Channel</th>
				<th>Result</th>
			</tr>
		</thead>
		<tbody>
			{#each notifications as n, i (i)}
				<tr>
					<td class="nowrap">{datetime(n.ts)}</td>
					<td class="nowrap">
						{KIND[n.kind]}{#if n.repeat}<span class="tag" title="Sent again while open: a repeat or an escalation">again</span>{/if}
					</td>
					<td class="nowrap"><Status state={n.state} /></td>
					<td class="nowrap">{n.contact}</td>
					<td class="nowrap hide-narrow">{n.channel || '–'}</td>
					<td class="info">
						{#if n.error}<span class="failed">FAILED</span> {n.error}{:else}<span class="sent">SENT</span>{/if}
					</td>
				</tr>
			{/each}
		</tbody>
	</table>
{/if}

<style>
	.tag {
		margin-left: 4px;
	}

	.sent {
		color: var(--ok);
		font-weight: 600;
	}

	.failed {
		color: var(--crit);
		font-weight: 600;
	}
</style>
