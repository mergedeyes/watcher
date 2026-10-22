<!-- Status totals per host or service group, like Nagios' group summary. -->
<script lang="ts">
	import { api, type HostView, type ServiceView } from './api.ts';
	import { live } from './live.svelte.ts';
	import Status from './Status.svelte';

	let { of }: { of: 'host' | 'service' } = $props();

	let groups = $state<{ name: string; title: string | null }[] | null>(null);

	$effect(() => {
		void live.reloads;
		api.groups().then((g) => (groups = of === 'host' ? g.hostgroups : g.servicegroups));
	});

	const STATES = ['critical', 'warning', 'unknown', 'ok'] as const;

	function hostsOf(group: string): HostView[] {
		return live.hosts.filter((h) => h.groups.includes(group));
	}

	function servicesOf(group: string): ServiceView[] {
		if (of === 'service') return live.services.filter((s) => s.groups.includes(group));
		const hosts = new Set(hostsOf(group).map((h) => h.name));
		return live.services.filter((s) => hosts.has(s.host));
	}

	function count<T>(list: T[], test: (t: T) => boolean) {
		return list.filter(test).length;
	}

	let list = $derived(of === 'host' ? '/hosts' : '/services');
</script>

{#if groups === null}
	<p class="notice">Loading…</p>
{:else}
	<section class="panel">
		<div class="panel-title">{of === 'host' ? 'Host' : 'Service'} Group Summary <span class="muted">{groups.length}</span></div>
		{#if groups.length === 0}
			<div class="empty">No {of} groups.</div>
		{:else}
			<table class="grid">
				<thead>
					<tr>
						<th>Group</th>
						{#if of === 'host'}<th>Hosts</th>{/if}
						<th>Services</th>
					</tr>
				</thead>
				<tbody>
					{#each groups as g (g.name)}
						{@const hosts = hostsOf(g.name)}
						{@const services = servicesOf(g.name)}
						{@const href = `${list}?group=${encodeURIComponent(g.name)}`}
						<tr>
							<td class="nowrap">
								<a {href}>{g.title ?? g.name}</a>
								{#if g.title}<span class="muted mono">({g.name})</span>{/if}
							</td>
							{#if of === 'host'}
								<td class="nowrap">
									{#if count(hosts, (h) => h.state === 'down')}<span class="pair"><Status state="down" /> {count(hosts, (h) => h.state === 'down')}</span>{/if}
									<span class="pair"><Status state="up" /> {count(hosts, (h) => h.state === 'up')}</span>
									{#if count(hosts, (h) => h.state === null)}<span class="pair"><Status state={null} /> {count(hosts, (h) => h.state === null)}</span>{/if}
								</td>
							{/if}
							<td class="nowrap">
								{#each STATES as st (st)}
									{@const n = count(services, (s) => s.state === st)}
									{#if n > 0 || st === 'ok'}<span class="pair"><Status state={st} /> {n}</span>{/if}
								{/each}
								{#if count(services, (s) => s.state === null)}<span class="pair"><Status state={null} /> {count(services, (s) => s.state === null)}</span>{/if}
							</td>
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
