<script lang="ts">
	import { page } from '$app/state';
	import { actions } from '#lib/actions.svelte.ts';
	import { api, hostHref, type HostCounts, type HostQuery } from '#lib/api.ts';
	import { auth } from '#lib/auth.svelte.ts';
	import { editHref } from '#lib/config.svelte.ts';
	import ConfigTree from '#lib/ConfigTree.svelte';
	import Flags from '#lib/Flags.svelte';
	import { count, datetime, since } from '#lib/format.ts';
	import { Debounced, Paging, ServerList, checkId, patchHost } from '#lib/list.svelte.ts';
	import { live } from '#lib/live.svelte.ts';
	import Pager from '#lib/Pager.svelte';
	import { Sorter } from '#lib/sort.svelte.ts';
	import Status from '#lib/Status.svelte';
	import Totals from '#lib/Totals.svelte';

	type Key = NonNullable<HostQuery['sort']>;
	const sorter = new Sorter<Key>('host');
	const search = new Debounced();

	const STATES = ['up', 'down', 'pending'];
	let filter = $derived.by(() => {
		const s = page.url.searchParams.get('state') ?? '';
		return STATES.includes(s) ? s : null;
	});
	/** Only hosts of this host group. */
	let group = $derived(page.url.searchParams.get('group'));

	let query = $derived<HostQuery>({ state: filter ?? undefined, group: group ?? undefined, q: search.settled || undefined });
	const paging = new Paging(() => JSON.stringify([query, sorter.key, sorter.desc]));
	const list = new ServerList(
		() => ({ ...query, sort: sorter.key, desc: sorter.desc, offset: paging.offset, limit: paging.limit }),
		api.hosts,
		{
			watch: (d) => ({ services: d.hosts.map(checkId).filter((id) => id !== null) }),
			patch: (d, view) => {
				for (const h of d.hosts) patchHost(h, view);
			}
		}
	);

	let counts = $derived<HostCounts>(list.data?.counts ?? { up: 0, down: 0, pending: 0 });
	let all = $derived(counts.up + counts.down + counts.pending);

	/** Totals links keep the group. */
	function href(state?: string) {
		const params = new URLSearchParams();
		if (group) params.set('group', group);
		if (state) params.set('state', state);
		const q = params.toString();
		return q ? `/hosts?${q}` : '/hosts';
	}

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
		<span class="head-actions">
			{#if group && auth.admin}
				<a class="btn" href={editHref('hostgroups', group)}>Edit Config</a>
				<ConfigTree kind="hostgroup" name={group} />
			{/if}
			<input type="search" placeholder="Search name, address, group" bind:value={search.value} aria-label="Search" />
		</span>
	</div>

	<div class="totals">
		<Totals
			title="Host Status Totals"
			items={[
				{ key: 'all', label: 'All', count: all, href: href() },
				{ key: 'up', label: 'Up', count: counts.up, href: href('up'), state: 'ok' },
				{ key: 'down', label: 'Down', count: counts.down, href: href('down'), state: 'critical' },
				{ key: 'pending', label: 'Pending', count: counts.pending, href: href('pending') }
			]}
		/>
	</div>

	{#if list.error && !list.data}
		<p class="notice error">{list.error}</p>
	{:else if !list.data}
		<p class="notice">Loading…</p>
	{:else}
		<section class="panel">
			<div class="panel-title">Host Status Details <span class="muted">{count(list.data.total)}</span></div>
			{#if list.data.total === 0}
				<div class="empty">{all === 0 && !group && !search.settled ? 'No hosts configured.' : 'No matches.'}</div>
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
						{#each list.data.hosts as h (h.name)}
							<tr data-host={h.state} oncontextmenu={(e) => actions.open(e, { host: h })}>
								<td class="nowrap">
									<a href={hostHref(h.name)}>{h.name}</a>
									{#if !h.enabled}<span class="tag">disabled</span>{/if}
								</td>
								<td class="nowrap">
									<Status state={h.state} />
									{#if h.flapping}<span class="tag flap">FLAPPING</span>{/if}
									<Flags v={h} />
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
				<Pager {paging} total={list.data.total} />
			{/if}
		</section>
	{/if}
</div>

<style>
	.totals {
		margin-bottom: 16px;
	}

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
