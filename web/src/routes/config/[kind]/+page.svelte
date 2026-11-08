<script lang="ts">
	import { page } from '$app/state';
	import type { ConfigObject } from '#lib/api.ts';
	import { config, editHref, fileHref, objectId } from '#lib/config.svelte.ts';
	import ConfigHead from '#lib/ConfigHead.svelte';
	import { live } from '#lib/live.svelte.ts';
	import { RELATIONS, remote } from '#lib/membership.ts';
	import { KINDS } from '#lib/schema.ts';

	let kind = $derived(KINDS.find((k) => k.path === page.params.kind));
	let query = $state('');

	$effect(() => {
		void live.reloads;
		config.load();
	});

	function list(v: unknown): string {
		if (v === undefined || v === null) return '';
		return Array.isArray(v) ? v.join(', ') : String(v);
	}

	let rows = $derived.by(() => {
		if (!kind) return [];
		const q = query.trim().toLowerCase();
		return config
			.of(kind.kind)
			.filter((o) => !q || JSON.stringify(o.fields).toLowerCase().includes(q))
			.sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true }));
	});

	function keys(o: ConfigObject) {
		return Object.keys(o.fields)
			.filter((k) => k !== 'name')
			.join(', ');
	}

	/** Schedule windows in one line: `sun 02:00-04:00; daily 12:00-12:30`. */
	function windows(v: unknown): string {
		if (!Array.isArray(v)) return '';
		return v
			.map((w) => {
				const t = (w ?? {}) as Record<string, unknown>;
				const days = Array.isArray(t.days) && t.days.length ? t.days.join(', ') : String(t.from ?? '').includes('-') ? '' : 'daily';
				return `${days} ${t.from ?? ''}-${t.to ?? ''}`.trim();
			})
			.join('; ');
	}

	/** A selector in one line: `all`, a list, or `groups: prod; node: dc1`. */
	function selector(v: unknown): string {
		if (v === undefined || v === null) return '';
		if (Array.isArray(v)) return v.join(', ');
		if (typeof v !== 'object') return String(v);
		const entries = Object.entries(v as Record<string, unknown>);
		if (entries.length === 0) return 'all';
		return entries
			.map(([k, x]) => `${k}: ${x !== null && typeof x === 'object' && !Array.isArray(x) ? selector(x) : list(x)}`)
			.join('; ');
	}

</script>

<!-- A membership list: what the object lists, then (muted) what lists it on the other side. -->
{#snippet both(o: ConfigObject)}
	{@const rel = RELATIONS[o.kind]}
	{#if rel}
		{@const own = list(o.fields[rel.field])}
		{own}{#each remote(config.objects, o.kind, o.name, own.split(', ').filter(Boolean)) as r, i (r.name)}{own || i ? ', ' : ''}<span
				class="muted" title={r.pattern ? `${rel.other} ${r.name}: pattern ${r.pattern}` : `listed on ${rel.other} ${r.name}`}>{r.name}</span
			>{/each}
	{/if}
{/snippet}

<div class="content">
	{#if !kind}
		<p class="notice">Unknown object type.</p>
	{:else}
		<div class="page-head">
			<h1>{kind.label} <span class="muted">{rows.length}</span></h1>
			<div class="actions">
				<input type="search" placeholder="Search" bind:value={query} aria-label="Search" />
				{#if config.writable}
					<a class="btn primary" href={editHref(kind.path)}>Add {kind.one}</a>
				{/if}
			</div>
		</div>
		<ConfigHead />

		{#if config.loaded}
			<section class="panel">
				<table class="grid">
					<thead>
						<tr>
							<th>Name</th>
							{#if kind.kind === 'host'}
								<th>Address</th>
								<th>Host Groups</th>
								<th class="hide-narrow">Services</th>
							{:else if kind.kind === 'service'}
								<th>Check</th>
								<th class="hide-narrow">Service Groups</th>
							{:else if kind.kind === 'check'}
								<th>Command</th>
							{:else if kind.kind === 'hostgroup'}
								<th>Title</th>
								<th>Members</th>
								<th class="hide-narrow">Services</th>
							{:else if kind.kind === 'servicegroup'}
								<th>Title</th>
								<th>Services</th>
							{:else if kind.kind === 'contactgroup'}
								<th>Title</th>
								<th>Members</th>
							{:else if kind.kind === 'downtimegroup'}
								<th>Title</th>
								<th>Hosts</th>
								<th>Schedule</th>
							{:else if kind.kind === 'contact'}
								<th>Email</th>
								<th class="hide-narrow">Webhook</th>
								<th class="hide-narrow">Groups</th>
							{:else if kind.kind === 'notify'}
								<th>To</th>
								<th>Match</th>
								<th class="hide-narrow">States</th>
							{:else}
								<th>Fields</th>
							{/if}
							<th class="hide-narrow">File</th>
							{#if config.writable}<th></th>{/if}
						</tr>
					</thead>
					<tbody>
						{#each rows as o, i (i)}
							{@const f = o.fields}
							<tr class:disabled={f.enabled === false}>
								<td class="nowrap">
									{#if o.name}<a href={editHref(kind.path, o.name)}>{o.name}</a>{:else}<a class="muted" href={editHref(kind.path, objectId(o))}>(unnamed)</a>{/if}
								</td>
								{#if kind.kind === 'host'}
									<td class="mono">{f.address ?? ''}</td>
									<td>{@render both(o)}</td>
									<td class="hide-narrow">{[list(f.services), list(f.servicegroups)].filter(Boolean).join(' · ')}</td>
								{:else if kind.kind === 'service'}
									<td class="mono info">{f.passive === true ? 'passive' : f.command !== undefined ? list(f.command) : list(f.check ?? o.name)}</td>
									<td class="hide-narrow">{@render both(o)}</td>
								{:else if kind.kind === 'check'}
									<td class="mono info">{f.passive === true ? 'passive' : list(f.command)}</td>
								{:else if kind.kind === 'hostgroup'}
									<td>{list(f.title)}</td>
									<td class="mono">{@render both(o)}</td>
									<td class="hide-narrow">{[list(f.services), list(f.servicegroups)].filter(Boolean).join(' · ')}</td>
								{:else if kind.kind === 'servicegroup'}
									<td>{list(f.title)}</td>
									<td class="mono">{@render both(o)}</td>
								{:else if kind.kind === 'contactgroup'}
									<td>{list(f.title)}</td>
									<td class="mono">{@render both(o)}</td>
								{:else if kind.kind === 'downtimegroup'}
									<td>{list(f.title)}</td>
									<td class="mono">{[list(f.members), list(f.hostgroups)].filter(Boolean).join(' · ')}</td>
									<td class="mono">{windows(f.schedule)}</td>
								{:else if kind.kind === 'contact'}
									<td>{list(f.email)}</td>
									<td class="mono hide-narrow info">{list(f.webhook)}</td>
									<td class="hide-narrow">{@render both(o)}</td>
								{:else if kind.kind === 'notify'}
									<td>{[list(f.contacts), list(f.contact_groups)].filter(Boolean).join(', ')}</td>
									<td class="mono">{[f.hosts !== undefined ? `hosts ${selector(f.hosts)}` : '', f.services !== undefined ? `services ${selector(f.services)}` : ''].filter(Boolean).join(' · ') || 'all'}</td>
									<td class="hide-narrow">{list(f.states) || 'all'}</td>
								{:else}
									<td class="mono">{keys(o)}</td>
								{/if}
								<td class="nowrap hide-narrow"><a class="mono" href={fileHref(o.path, o.line)}>{o.path}:{o.line}</a></td>
								{#if config.writable}
									<td class="nowrap"><a href="/config/{kind.path}/edit?clone={encodeURIComponent(objectId(o))}">Clone</a></td>
								{/if}
							</tr>
						{:else}
							<tr><td colspan="7" class="empty">No {kind.label.toLowerCase()}.</td></tr>
						{/each}
					</tbody>
				</table>
			</section>
		{/if}
	{/if}
</div>

<style>
	.actions {
		display: flex;
		gap: 8px;
		align-items: center;
	}

	tr.disabled td {
		color: var(--muted);
		text-decoration: line-through;
	}
</style>
