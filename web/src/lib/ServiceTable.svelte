<!--
	Service status details, as in Nagios: one row per service, the host name
	only on the first row of each host when sorted by host.
-->
<script lang="ts">
	import { untrack } from 'svelte';
	import { actions } from './actions.svelte.ts';
	import { hostHref, serviceHref, type ServiceView } from './api.ts';
	import Flags from './Flags.svelte';
	import { datetime, severity, since } from './format.ts';
	import { live } from './live.svelte.ts';
	import { Sorter } from './sort.svelte.ts';
	import Status from './Status.svelte';

	type Key = 'host' | 'service' | 'status' | 'last_check' | 'duration' | 'attempt';

	interface Props {
		rows: ServiceView[];
		showHost?: boolean;
		/** Initial sort column. */
		sort?: Key;
		empty?: string;
	}

	let { rows, showHost = true, sort = 'host', empty = 'No services.' }: Props = $props();

	// The initial sort only; the user changes it afterwards.
	const sorter = untrack(() => new Sorter<Key>(sort, sort === 'status'));
	const time = (s: string | null) => (s ? Date.parse(s) : 0);
	const text = (a: string, b: string) => a.localeCompare(b, undefined, { numeric: true });

	let sorted = $derived(
		sorter.apply(rows, {
			host: (a, b) => text(a.host, b.host),
			service: (a, b) => text(a.title ?? a.service, b.title ?? b.service),
			status: (a, b) => severity(a.state) - severity(b.state),
			last_check: (a, b) => time(a.last_check) - time(b.last_check),
			// Longer duration = earlier change.
			duration: (a, b) => time(b.last_change) - time(a.last_change),
			attempt: (a, b) => (a.attempt ?? 0) - (b.attempt ?? 0)
		})
	);

	const COLUMNS: [Key, string, string][] = [
		['host', 'Host', ''],
		['service', 'Service', ''],
		['status', 'Status', ''],
		['last_check', 'Last Check', 'hide-narrow'],
		['duration', 'Duration', 'hide-narrow'],
		['attempt', 'Attempt', 'hide-narrow']
	];
	let columns = $derived(COLUMNS.filter(([key]) => showHost || key !== 'host'));
</script>

{#if rows.length === 0}
	<div class="empty">{empty}</div>
{:else}
	<table class="grid">
		<thead>
			<tr>
				{#each columns as [key, label, cls] (key)}
					<th class={cls}>
						<button type="button" onclick={() => sorter.toggle(key)}>{label}{sorter.mark(key)}</button>
					</th>
				{/each}
				<th>Status Information</th>
			</tr>
		</thead>
		<tbody>
			{#each sorted as s, i (`${s.host}/${s.service}`)}
				<tr data-state={s.state} oncontextmenu={(e) => actions.open(e, { service: s })}>
					{#if showHost}
						<td class="nowrap">
							{#if sorter.key !== 'host' || i === 0 || sorted[i - 1]?.host !== s.host}
								<a href={hostHref(s.host)}>{s.host}</a>
							{/if}
						</td>
					{/if}
					<td class="nowrap">
						<a href={serviceHref(s.host, s.service)} title={s.title ? s.service : undefined}>{s.title ?? s.service}</a>
						{#if s.passive}<span class="tag">passive</span>{/if}
					</td>
					<td class="nowrap">
						<Status state={s.state} />
						{#if s.flapping}<span class="tag flap" title="State changes too often; notifications held back">FLAPPING</span>{/if}
						<Flags v={s} />
					</td>
					<td class="nowrap hide-narrow">{datetime(s.last_check)}</td>
					<td class="nowrap hide-narrow">{since(s.last_change, live.now)}</td>
					<td class="nowrap hide-narrow">
						{#if s.attempt !== null}{s.attempt}/{s.max_attempts}{#if s.state_type === 'soft'}&nbsp;<span class="tag">SOFT</span>{/if}{/if}
					</td>
					<td class="info">{s.output}</td>
				</tr>
			{/each}
		</tbody>
	</table>
{/if}
