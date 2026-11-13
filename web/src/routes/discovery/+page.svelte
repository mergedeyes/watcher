<script lang="ts">
	import { api, type DiscoveredGuest, type Discovery } from '#lib/api.ts';
	import { actions } from '#lib/actions.svelte.ts';
	import { auth } from '#lib/auth.svelte.ts';
	import { editHref } from '#lib/config.svelte.ts';
	import { age } from '#lib/format.ts';
	import { live } from '#lib/live.svelte.ts';

	let list = $state<Discovery[]>([]);
	let loaded = $state(false);
	let error = $state<string | null>(null);
	let busy = $state(false);

	$effect(() => {
		void live.reloads;
		if (auth.admin) void load();
	});

	// Runs take a while: look again while one is going on.
	$effect(() => {
		if (!list.some((d) => d.running)) return;
		const t = setTimeout(load, 3000);
		return () => clearTimeout(t);
	});

	async function load() {
		try {
			list = await api.discoveries();
			error = null;
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			loaded = true;
		}
	}

	async function act(d: Discovery, action: 'accept' | 'remove' | 'ignore' | 'unignore', guests: DiscoveredGuest[], done: string) {
		if (guests.length === 0) return;
		busy = true;
		try {
			await api.discoveryAction(d.name, action, guests.map((g) => g.vmid));
			actions.say(`${done}: ${guests.map((g) => g.host ?? g.name).join(', ')}`);
			await load();
		} catch (e) {
			actions.say(e instanceof Error ? e.message : String(e), true);
		} finally {
			busy = false;
		}
	}

	async function run(d: Discovery) {
		try {
			await api.runDiscovery(d.name);
			d.running = true;
		} catch (e) {
			actions.say(e instanceof Error ? e.message : String(e), true);
		}
	}

	function offered(d: Discovery) {
		return d.guests.filter((g) => !g.host && !g.ignored);
	}

	function remove(d: Discovery, g: DiscoveredGuest) {
		if (confirm(`Remove host ${g.host}? Its history stays; the guest is offered again.`)) void act(d, 'remove', [g], 'Removed');
	}
</script>

<div class="content">
	<div class="page-head"><h1>Discovery</h1></div>
	{#if !auth.admin}
		<p class="notice">Admins only.</p>
	{:else}
		{#if error}<p class="notice error">{error}</p>{/if}
		{#if loaded && list.length === 0 && !error}
			<p class="notice">
				No Proxmox VE host looks for VMs and containers yet. Check <b>Discover</b> in the Proxmox VE section of a host that has a
				Proxmox secret (<a href="/config/hosts">Configuration → Hosts</a>).
			</p>
		{/if}

		{#each list as d (d.name)}
			{@const fresh = offered(d)}
			<section class="panel">
				<div class="panel-title">
					<span><a href={editHref('hosts', d.name)}>{d.name}</a> <span class="muted mono">{d.url}</span></span>
					<span class="head">
						<span class="muted">
							{#if d.running}running…{:else if d.last_run}last run {age(d.last_run, live.now)} ago{:else}not run yet{/if}
						</span>
						<button type="button" class="btn small" onclick={() => run(d)} disabled={d.running}>Run Now</button>
						{#if fresh.length > 1}
							<button type="button" class="btn small primary" disabled={busy} onclick={() => act(d, 'accept', fresh, 'Added')}
								>Accept All New ({fresh.length})</button
							>
						{/if}
					</span>
				</div>
				{#if d.error}<p class="body error">{d.error}</p>{/if}
				{#if d.guests.length === 0}
					<p class="body muted">{d.last_run ? 'No VMs or containers found.' : 'Nothing found yet.'}</p>
				{:else}
					<table class="grid">
						<thead>
							<tr>
								<th>Name</th>
								<th>Type</th>
								<th>VM ID</th>
								<th class="hide-narrow">Node</th>
								<th>Address</th>
								<th class="hide-narrow">State</th>
								<th>Host</th>
								<th></th>
							</tr>
						</thead>
						<tbody>
							{#each d.guests as g (g.vmid)}
								<tr class:ignored={g.ignored}>
									<td class="nowrap">{g.name}</td>
									<td>{g.kind === 'lxc' ? 'CT' : 'VM'}</td>
									<td class="mono">{g.vmid}</td>
									<td class="hide-narrow">{g.node}</td>
									<td class="mono">{g.address ?? '—'}</td>
									<td class="hide-narrow">{g.running ? 'running' : 'stopped'}</td>
									<td class="nowrap">
										{#if g.host}<a href="/hosts/{encodeURIComponent(g.host)}">{g.host}</a>
										{:else if g.ignored}<span class="muted">ignored</span>
										{:else}<span class="tag new">NEW</span>{/if}
									</td>
									<td class="nowrap actions">
										{#if g.host}
											<button type="button" class="btn small danger" disabled={busy} onclick={() => remove(d, g)}>Remove</button>
										{:else if g.ignored}
											<button type="button" class="btn small" disabled={busy} onclick={() => act(d, 'unignore', [g], 'Offered again')}>Restore</button>
										{:else}
											<button type="button" class="btn small primary" disabled={busy} onclick={() => act(d, 'accept', [g], 'Added')}>Accept</button>
											<button type="button" class="btn small" disabled={busy} onclick={() => act(d, 'ignore', [g], 'Ignored')}>Ignore</button>
										{/if}
									</td>
								</tr>
							{/each}
						</tbody>
					</table>
				{/if}
				<p class="body muted hint">
					Accepted guests are written to <span class="mono">{d.file}</span> with the check
					<span class="mono">proxmox-guest</span>; deleted ones disappear from it on their own.
				</p>
			</section>
		{/each}
	{/if}
</div>

<style>
	.panel + .panel {
		margin-top: 12px;
	}

	.panel-title {
		display: flex;
		justify-content: space-between;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
	}

	.head {
		display: flex;
		gap: 8px;
		align-items: center;
		font-weight: normal;
	}

	.body {
		margin: 0;
		padding: 10px 12px;
	}

	.hint {
		font-size: 12px;
	}

	.error {
		color: var(--crit);
	}

	.actions {
		text-align: right;
	}

	.btn.small {
		font-size: 12px;
		padding: 2px 8px;
	}

	.btn.danger {
		color: var(--crit);
		border-color: var(--crit);
	}

	tr.ignored td {
		color: var(--muted);
	}

	.tag.new {
		font-weight: 600;
		color: var(--link);
		border-color: var(--link);
	}
</style>
