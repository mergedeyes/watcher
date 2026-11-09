<!-- Status totals per host or service group, like Nagios' group summary. -->
<script lang="ts">
	import { count } from './format.ts';
	import { api, type Counts, type HostCounts } from './api.ts';
	import { live } from './live.svelte.ts';
	import { auth } from './auth.svelte.ts';
	import { editHref } from './config.svelte.ts';
	import Status from './Status.svelte';

	let { of }: { of: 'host' | 'service' } = $props();

	interface Row {
		name: string;
		title: string | null;
		hosts?: HostCounts;
		services: Counts;
	}

	let groups = $state<Row[] | null>(null);

	// Totals change with states: refetch with the generation.
	$effect(() => {
		void live.reloads;
		void live.generation;
		api.groups().then((g) => (groups = of === 'host' ? g.hostgroups : g.servicegroups));
	});

	const STATES = ['critical', 'warning', 'unknown', 'ok'] as const;

	let list = $derived(of === 'host' ? '/hosts' : '/services');
</script>

{#if groups === null}
	<p class="notice">Loading…</p>
{:else}
	<section class="panel">
		<div class="panel-title">{of === 'host' ? 'Host' : 'Service'} Group Summary <span class="muted">{count(groups.length)}</span></div>
		{#if groups.length === 0}
			<div class="empty">No {of} groups.</div>
		{:else}
			<table class="grid">
				<thead>
					<tr>
						<th>Group</th>
						{#if of === 'host'}<th>Hosts</th>{/if}
						<th>Services</th>
						{#if auth.admin}<th></th>{/if}
					</tr>
				</thead>
				<tbody>
					{#each groups as g (g.name)}
						{@const hosts = g.hosts ?? { up: 0, down: 0, pending: 0 }}
						{@const services = g.services}
						{@const href = `${list}?group=${encodeURIComponent(g.name)}`}
						<tr>
							<td class="nowrap">
								<a {href}>{g.title ?? g.name}</a>
								{#if g.title}<span class="muted mono">({g.name})</span>{/if}
							</td>
							{#if of === 'host'}
								<td class="nowrap">
									{#if hosts.down}<span class="pair"><Status state="down" /> {count(hosts.down)}</span>{/if}
									<span class="pair"><Status state="up" /> {count(hosts.up)}</span>
									{#if hosts.pending}<span class="pair"><Status state={null} /> {count(hosts.pending)}</span>{/if}
								</td>
							{/if}
							<td class="nowrap">
								{#each STATES as st (st)}
									{@const n = services[st]}
									{#if n > 0 || st === 'ok'}<span class="pair"><Status state={st} /> {count(n)}</span>{/if}
								{/each}
								{#if services.pending}<span class="pair"><Status state={null} /> {count(services.pending)}</span>{/if}
							</td>
							{#if auth.admin}
								<td class="nowrap"><a href={editHref(of === 'host' ? 'hostgroups' : 'servicegroups', g.name)}>Edit Config</a></td>
							{/if}
						</tr>
					{/each}
				</tbody>
			</table>
		{/if}
	</section>
{/if}

<style>
	.pair {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		margin-right: 12px;
	}

	.pair :global(.st) {
		min-width: 64px;
	}

	.muted {
		margin-left: 4px;
		font-size: 11px;
	}
</style>
