<script lang="ts">
	import { onMount } from 'svelte';
	import { api, type ApiToken } from '#lib/api.ts';
	import { auth } from '#lib/auth.svelte.ts';
	import { datetime } from '#lib/format.ts';

	let tokens = $state<ApiToken[]>([]);
	let name = $state('');
	let created = $state<{ name: string; token: string } | null>(null);
	let error = $state<string | null>(null);
	let copied = $state(false);

	async function load() {
		tokens = await api.tokens();
	}

	onMount(() => {
		if (auth.status?.required) load();
	});

	async function create(event: SubmitEvent) {
		event.preventDefault();
		error = null;
		try {
			const result = await api.createToken(name.trim());
			created = { name: result.info.name, token: result.token };
			name = '';
			copied = false;
			await load();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	}

	async function revoke(token: ApiToken) {
		if (!confirm(`Revoke token "${token.name}"?`)) return;
		await api.deleteToken(token.id);
		if (created?.name === token.name) created = null;
		await load();
	}

	async function copy() {
		if (!created) return;
		await navigator.clipboard.writeText(created.token);
		copied = true;
	}
</script>

<div class="content">
	<div class="page-head"><h1>Account</h1></div>

	{#if !auth.status?.required}
		<p class="notice">Authentication disabled.</p>
	{:else}
		<section class="panel">
			<div class="panel-title">User</div>
			<table class="grid kv">
				<tbody>
					<tr><th>Username</th><td>{auth.status.user?.username}</td></tr>
					<tr><th>Admin</th><td>{auth.status.user?.admin ? 'yes' : 'no'}</td></tr>
				</tbody>
			</table>
		</section>

		<section class="panel">
			<div class="panel-title">API Tokens</div>
			<div class="body">
				{#if created}
					<div class="created">
						<div>Token "{created.name}" (shown once):</div>
						<div class="secret">
							<code>{created.token}</code>
							<button type="button" class="btn" onclick={copy}>{copied ? 'Copied' : 'Copy'}</button>
						</div>
						<pre class="mono example">curl -X POST {location.origin}/api/v1/services/HOST/SERVICE/result \
  -H 'Authorization: Bearer {created.token}' \
  -H 'content-type: application/json' \
  -d '&#123;"exit_code": 0, "output": "BACKUP OK"&#125;'</pre>
					</div>
				{/if}
				<form class="new" onsubmit={create}>
					<input placeholder="Name, e.g. backup-cron" bind:value={name} required maxlength="100" aria-label="Token name" />
					<button type="submit" class="btn primary">Create token</button>
				</form>
				{#if error}<p class="error" role="alert">{error}</p>{/if}
			</div>
			{#if tokens.length > 0}
				<table class="grid">
					<thead>
						<tr>
							<th>Name</th>
							<th class="hide-narrow">Created</th>
							<th>Last Used</th>
							<th></th>
						</tr>
					</thead>
					<tbody>
						{#each tokens as t (t.id)}
							<tr>
								<td>{t.name}</td>
								<td class="nowrap hide-narrow">{datetime(t.created)}</td>
								<td class="nowrap">{t.last_used ? datetime(t.last_used) : 'never'}</td>
								<td class="actions"><button type="button" class="btn" onclick={() => revoke(t)}>Revoke</button></td>
							</tr>
						{/each}
					</tbody>
				</table>
			{/if}
		</section>
	{/if}
</div>

<style>
	.kv th {
		width: 170px;
		background: var(--panel-alt);
		border-bottom: 1px solid var(--line);
	}

	.body {
		padding: 12px;
	}

	.created {
		margin-bottom: 12px;
		padding: 10px;
		border: 1px solid var(--ok);
		border-radius: var(--radius);
	}

	.secret {
		display: flex;
		gap: 8px;
		align-items: center;
		flex-wrap: wrap;
		margin: 6px 0;
	}

	.secret code {
		padding: 4px 8px;
		background: var(--panel-alt);
		border: 1px solid var(--line);
		border-radius: var(--radius);
		overflow-wrap: anywhere;
	}

	.example {
		margin: 6px 0 0;
		padding: 8px;
		background: var(--panel-alt);
		border-radius: var(--radius);
		overflow-x: auto;
	}

	.new {
		display: flex;
		gap: 8px;
		flex-wrap: wrap;
	}

	.new input {
		flex: 1;
		min-width: 220px;
	}

	.actions {
		text-align: right;
	}

	.error {
		color: var(--crit);
	}
</style>
