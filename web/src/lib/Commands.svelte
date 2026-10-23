<!-- The commands panel of a host or service page, as in Nagios; the same entries as the right-click menu. -->
<script lang="ts">
	import type { Flag } from './api.ts';
	import { actions, entries, type Target } from './actions.svelte.ts';
	import { datetime } from './format.ts';

	let { target, flags = [], downtimes = [] }: { target: Target; flags?: Flag[]; downtimes?: string[] } = $props();

	const KIND = { ack: 'Acknowledged', mute: 'Muted', no_checks: 'Checks off' } as const;
</script>

<section class="panel">
	<div class="panel-title">{target.service ? 'Service' : 'Host'} Commands</div>
	<div class="body">
		<ul>
			{#each entries(target) as entry (entry.label)}
				<li><button type="button" class="link" onclick={() => actions.choose(entry, target)}>{entry.label}</button></li>
			{/each}
		</ul>
		{#if flags.length || downtimes.length}
			<table class="grid kv">
				<tbody>
					{#each flags as f (`${f.kind}/${f.target}`)}
						<tr>
							<th>{KIND[f.kind]}</th>
							<td>
								{f.comment || '–'}
								<span class="muted">· {f.author}, {datetime(f.created)}{f.target.includes('/') ? '' : ` · host ${f.target}`}</span>
							</td>
						</tr>
					{/each}
					{#each downtimes as d, i (i)}
						<tr><th>In Downtime</th><td><a href="/downtimes">{d}</a></td></tr>
					{/each}
				</tbody>
			</table>
		{/if}
	</div>
</section>

<style>
	.body {
		display: flex;
		flex-wrap: wrap;
		gap: 4px 24px;
		padding: 8px 12px;
	}

	ul {
		margin: 0;
		padding: 0;
		list-style: none;
		columns: 2;
		column-gap: 24px;
		font-size: 13px;
	}

	li {
		padding: 2px 0;
	}

	.link {
		border: 0;
		padding: 0;
		background: none;
		color: var(--link);
		font: inherit;
		cursor: pointer;
	}

	.link:hover {
		text-decoration: underline;
	}

	table {
		flex: 1;
		min-width: 260px;
	}

	.kv th {
		width: 120px;
	}
</style>
