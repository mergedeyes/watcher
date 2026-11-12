<script lang="ts">
	import { api, type JoinToken, type Node } from '#lib/api.ts';
	import { actions } from '#lib/actions.svelte.ts';
	import { auth } from '#lib/auth.svelte.ts';
	import { datetime, since } from '#lib/format.ts';

	let nodes = $state<Node[]>([]);
	let error = $state<string | null>(null);
	let loaded = $state(false);
	let name = $state('');
	/** The join command just made, to copy to the node's machine. */
	let join = $state<JoinToken | null>(null);
	let copied = $state(false);
	let now = $state(Date.now());

	// Connections come and go: refresh while the page is open.
	$effect(() => {
		void load();
		const timer = setInterval(() => {
			now = Date.now();
			void load();
		}, 10_000);
		return () => clearInterval(timer);
	});

	async function load() {
		try {
			nodes = await api.nodes();
			error = null;
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
		loaded = true;
	}

	/** Gets a join command, shows it (that says it worked), reloads. */
	async function token(f: () => Promise<JoinToken>): Promise<boolean> {
		try {
			join = await f();
			copied = false;
			await load();
			return true;
		} catch (e) {
			actions.say(e instanceof Error ? e.message : String(e), true);
			return false;
		}
	}

	async function add(e: SubmitEvent) {
		e.preventDefault();
		const n = name.trim();
		if (await token(() => api.addNode(n))) name = '';
	}

	function rejoin(n: Node) {
		if (confirm(`New join command for ${n.name}? Its current certificate works until it joins again.`))
			void token(() => api.nodeToken(n.name));
	}

	async function remove(n: Node) {
		const used = n.hosts.length ? ` ${n.hosts.length} host(s) name it.` : '';
		if (!confirm(`Remove node ${n.name}? Its certificate stops working and its connection ends.${used}`)) return;
		try {
			await api.removeNode(n.name);
			actions.say(`Node ${n.name} removed`);
			if (join?.name === n.name) join = null;
			await load();
		} catch (e) {
			actions.say(e instanceof Error ? e.message : String(e), true);
		}
	}

	async function copy(text: string) {
		await navigator.clipboard.writeText(text);
		copied = true;
	}

	const STATE = {
		connected: { label: 'CONNECTED', st: 'ok' },
		offline: { label: 'OFFLINE', st: 'critical' },
		new: { label: 'NOT JOINED', st: 'pending' }
	} as const;

	/** At most this many host names per row. */
	const HOSTS = 3;

	let connected = $derived(nodes.filter((n) => n.state === 'connected').length);
</script>

<div class="content">
	<div class="page-head">
		<h1>Nodes <span class="muted">{#if nodes.length}{connected}/{nodes.length} connected{/if}</span></h1>
	</div>
	{#if error}
		<p class="notice">{error}</p>
	{:else if loaded}
		{#if auth.admin}
			<section class="panel">
				<div class="panel-title">Add Node</div>
				<div class="body">
					{#if join}
						<div class="created">
							<div>
								Run this on the machine of node {join.name} (works once, until {datetime(join.expires)}):
							</div>
							<div class="command">
								<code>{join.command}</code>
								<button type="button" class="btn" onclick={() => copy(join?.command ?? '')}>{copied ? 'Copied' : 'Copy'}</button>
							</div>
						</div>
					{/if}
					<form class="new" onsubmit={add}>
						<input placeholder="Name, e.g. site-a" bind:value={name} required maxlength="64" autocomplete="off" aria-label="Name" />
						<button type="submit" class="btn primary">Add</button>
					</form>
					<p class="muted hint">Hosts: <code>node = "NAME"</code></p>
				</div>
			</section>
		{/if}

		<section class="panel">
			{#if nodes.length === 0}
				<p class="empty">No nodes yet.</p>
			{:else}
				<table class="grid">
					<thead>
						<tr>
							<th>Node</th>
							<th>State</th>
							<th>Since</th>
							<th>Hosts</th>
							<th class="hide-narrow">Machine</th>
							<th class="hide-narrow">Version</th>
							{#if auth.admin}<th></th>{/if}
						</tr>
					</thead>
					<tbody>
						{#each nodes as n (n.name)}
							<tr>
								<td class="nowrap">{n.name}</td>
								<td><span class="st" data-state={STATE[n.state].st}>{STATE[n.state].label}</span></td>
								<td class="nowrap">
									{#if n.state === 'connected'}
										{since(n.connected_since, now)}
									{:else if n.state === 'offline'}
										<span class="muted">last seen</span> {n.last_seen ? datetime(n.last_seen) : 'never'}
									{:else if n.token_expires}
										<span class="muted">token until</span> {datetime(n.token_expires)}
									{/if}
								</td>
								<td>
									{#each n.hosts.slice(0, HOSTS) as h, i (h)}{#if i > 0}{', '}{/if}<a href="/hosts/{encodeURIComponent(h)}">{h}</a>{/each}
									{#if n.hosts.length > HOSTS}<span class="muted"> +{n.hosts.length - HOSTS}</span>{/if}
									{#if n.hosts.length === 0}<span class="muted">—</span>{/if}
								</td>
								<td class="nowrap hide-narrow">
									{n.hostname ?? ''}{#if n.address}{' '}<span class="muted">{n.address}</span>{/if}
								</td>
								<td class="hide-narrow">{n.version ?? ''}</td>
								{#if auth.admin}
									<td class="nowrap actions">
										{#if n.joined}
											<button type="button" class="btn small" onclick={() => rejoin(n)}>New Join Command</button>
										{:else}
											<button type="button" class="btn small" onclick={() => token(() => api.addNode(n.name))}>Join Command</button>
										{/if}
										{#if n.joined || n.token_expires}
											<button type="button" class="btn small danger" onclick={() => remove(n)}>Remove</button>
										{/if}
									</td>
								{/if}
							</tr>
						{/each}
					</tbody>
				</table>
			{/if}
		</section>
	{/if}
</div>

<style>
	.body {
		padding: 12px;
	}

	.new {
		display: flex;
		gap: 8px;
		flex-wrap: wrap;
		align-items: center;
	}

	.new input {
		flex: 1;
		min-width: 180px;
		max-width: 360px;
	}

	.hint {
		margin: 8px 0 0;
		font-size: 12px;
	}

	.created {
		margin-bottom: 12px;
		padding: 10px;
		border: 1px solid var(--ok);
		border-radius: var(--radius);
	}

	.command {
		display: flex;
		gap: 8px;
		align-items: center;
		flex-wrap: wrap;
		margin: 6px 0 0;
	}

	.command code {
		padding: 4px 8px;
		background: var(--panel-alt);
		border: 1px solid var(--line);
		border-radius: var(--radius);
		overflow-wrap: anywhere;
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
</style>
