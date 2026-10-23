<script lang="ts">
	import { page } from '$app/state';
	import { hostHref, type HostView } from '#lib/api.ts';
	import { datetime, hostSeverity, since } from '#lib/format.ts';
	import { live } from '#lib/live.svelte.ts';
	import { Sorter } from '#lib/sort.svelte.ts';
	import Status from '#lib/Status.svelte';

	type Key = 'host' | 'status' | 'address' | 'last_check' | 'duration';
	const sorter = new Sorter<Key>('host');
	let query = $state('');

	let filter = $derived(page.url.searchParams.get('state'));
	/** Only hosts of this host group. */
	let group = $derived(page.url.searchParams.get('group'));

	function matches(h: HostView) {
		switch (filter) {
			case 'up':
				return h.state === 'up';
			case 'down':
				return h.state === 'down';
			case 'pending':
				return h.state === null;
			default:
				return true;
		}
	}

	let shown = $derived.by(() => {
		const q = query.trim().toLowerCase();
		const rows = live.hosts.filter(
			(h) =>
				matches(h) &&
				(!group || h.groups.includes(group)) &&
				(!q || [h.name, h.address, ...h.groups].some((v) => v.toLowerCase().includes(q)))
		);
		const text = (a: string, b: string) => a.localeCompare(b, undefined, { numeric: true });
		return sorter.apply(rows, {
			host: (a, b) => text(a.name, b.name),
			status: (a, b) => hostSeverity(a.state) - hostSeverity(b.state),
			address: (a, b) => text(a.address, b.address),
			last_check: (a, b) => (a.last_check ?? '').localeCompare(b.last_check ?? ''),
			duration: (a, b) => (b.last_change ?? '').localeCompare(a.last_change ?? '')
		});
	});

	const COUNTS = [
		['ok', 'OK'],
		['warning', 'W'],
		['critical', 'C'],
		['unknown', 'U'],
		['pending', 'P']
	] as const;
</script>

<div class="content">
	<div class="page-head">
		<h1>Hosts{group ? ` · ${group}` : ''}{filter ? ` · ${filter.toUpperCase()}` : ''}</h1>
		<input type="search" placeholder="Search name, address, group" bind:value={query} aria-label="Search" />
	</div>

	{#if !live.loaded}
		<p class="notice">Loading…</p>
	{:else}
		<section class="panel">
			<div class="panel-title">Host Status Details <span class="muted">{shown.length}/{live.hosts.length}</span></div>
			{#if shown.length === 0}
				<div class="empty">{live.hosts.length === 0 ? 'No hosts configured.' : 'No matches.'}</div>
			{:else}
				<table class="grid">
					<thead>
						<tr>
							<th><button type="button" onclick={() => sorter.toggle('host')}>Host{sorter.mark('host')}</button></th>
							<th><button type="button" onclick={() => sorter.toggle('status')}>Status{sorter.mark('status')}</button></th>
							<th class="hide-narrow"><button type="button" onclick={() => sorter.toggle('address')}>Address{sorter.mark('address')}</button></th>
							<th class="hide-narrow"><button type="button" onclick={() => sorter.toggle('last_check')}>Last Check{sorter.mark('last_check')}</button></th>
							<th class="hide-narrow"><button type="button" onclick={() => sorter.toggle('duration')}>Duration{sorter.mark('duration')}</button></th>
							<th>Services</th>
							<th>Status Information</th>
						</tr>
					</thead>
					<tbody>
						{#each shown as h (h.name)}
							<tr data-host={h.state}>
								<td class="nowrap">
									<a href={hostHref(h.name)}>{h.name}</a>
									{#if !h.enabled}<span class="tag">disabled</span>{/if}
								</td>
								<td class="nowrap">
									<Status state={h.state} />
									{#if h.flapping}<span class="tag flap">FLAPPING</span>{/if}
								</td>
								<td class="nowrap hide-narrow mono">{h.address}</td>
								<td class="nowrap hide-narrow">{datetime(h.last_check)}</td>
								<td class="nowrap hide-narrow">{since(h.last_change, live.now)}</td>
								<td class="nowrap">
									{#each COUNTS as [key, label] (key)}
										{#if h.services[key] > 0}<span class="count" data-state={key}>{h.services[key]} {label}</span>{/if}
									{/each}
								</td>
								<td class="info">{h.check ? h.output : ''}{#if !h.check}<span class="muted">no host check</span>{/if}</td>
							</tr>
						{/each}
					</tbody>
				</table>
			{/if}
		</section>
	{/if}
</div>

<style>
	.count {
		display: inline-block;
		margin-right: 4px;
		padding: 0 5px;
		border-radius: 3px;
		font-size: 11px;
		font-weight: 600;
		color: #fff;
	}
	.count[data-state='ok'] {
		background: var(--ok);
	}
	.count[data-state='warning'] {
		background: var(--warn);
		color: var(--on-warn);
	}
	.count[data-state='critical'] {
		background: var(--crit);
	}
	.count[data-state='unknown'] {
		background: var(--unknown);
	}
	.count[data-state='pending'] {
		background: var(--pending);
	}
</style>
