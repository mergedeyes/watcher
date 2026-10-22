<script lang="ts">
	import { page } from '$app/state';
	import type { ConfigObject } from '#lib/api.ts';
	import { config, editHref, fileHref } from '#lib/config.svelte.ts';
	import ConfigHead from '#lib/ConfigHead.svelte';
	import { live } from '#lib/live.svelte.ts';
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

	/** A selector in one line: `all`, a list, or `tags: prod; node: dc1`. */
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

	function services(v: unknown): string {
		return v !== null && typeof v === 'object' ? Object.keys(v).join(', ') : '';
	}
</script>

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
								<th>Roles</th>
								<th class="hide-narrow">Tags</th>
							{:else if kind.kind === 'role'}
								<th>Services</th>
								<th class="hide-narrow">Hosts</th>
								<th class="hide-narrow">Include</th>
							{:else if kind.kind === 'check'}
								<th>Command</th>
							{:else if kind.kind === 'hostgroup' || kind.kind === 'servicegroup' || kind.kind === 'contactgroup'}
								<th>Title</th>
								<th>Members</th>
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
						</tr>
					</thead>
					<tbody>
						{#each rows as o (`${o.path}:${o.line}`)}
							{@const f = o.fields}
							<tr class:disabled={f.enabled === false}>
								<td class="nowrap">
									{#if o.name}<a href={editHref(kind.path, o.name)}>{o.name}</a>{:else}<a class="muted" href={fileHref(o.path, o.line)}>(unnamed)</a>{/if}
								</td>
								{#if kind.kind === 'host'}
									<td class="mono">{f.address ?? ''}</td>
									<td>{list(f.roles)}</td>
									<td class="hide-narrow">
										{#each Array.isArray(f.tags) ? f.tags : [] as tag (tag)}<span class="tag">{tag}</span>{' '}{/each}
									</td>
								{:else if kind.kind === 'role'}
									<td class="mono">{services(f.service)}</td>
									<td class="mono hide-narrow">{selector(f.hosts)}</td>
									<td class="hide-narrow">{list(f.include)}</td>
								{:else if kind.kind === 'check'}
									<td class="mono info">{f.passive === true ? 'passive' : list(f.command)}</td>
								{:else if kind.kind === 'hostgroup' || kind.kind === 'servicegroup' || kind.kind === 'contactgroup'}
									<td>{list(f.title)}</td>
									<td class="mono">{selector(f.members)}</td>
								{:else if kind.kind === 'contact'}
									<td>{list(f.email)}</td>
									<td class="mono hide-narrow info">{list(f.webhook)}</td>
									<td class="hide-narrow">{list(f.groups)}</td>
								{:else if kind.kind === 'notify'}
									<td>{[list(f.contacts), list(f.contact_groups)].filter(Boolean).join(', ')}</td>
									<td class="mono">{[f.hosts !== undefined ? `hosts ${selector(f.hosts)}` : '', f.services !== undefined ? `services ${selector(f.services)}` : ''].filter(Boolean).join(' · ') || 'all'}</td>
									<td class="hide-narrow">{list(f.states) || 'all'}</td>
								{:else}
									<td class="mono">{keys(o)}</td>
								{/if}
								<td class="nowrap hide-narrow"><a class="mono" href={fileHref(o.path, o.line)}>{o.path}:{o.line}</a></td>
							</tr>
						{:else}
							<tr><td colspan="6" class="empty">No {kind.label.toLowerCase()}.</td></tr>
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
