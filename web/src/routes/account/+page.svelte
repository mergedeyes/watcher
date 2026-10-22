<script lang="ts">
	import { onMount } from 'svelte';
	import { api, type ApiToken } from '#lib/api.ts';
	import { auth } from '#lib/auth.svelte.ts';
	import { timestamp } from '#lib/format.ts';

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
		if (!confirm(`Revoke the token “${token.name}”? Scripts using it stop working.`)) return;
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

<main class="page">
	<h1 class="legend">Account</h1>

	{#if !auth.status?.required}
		<p class="notice">This server runs without logins, so there are no accounts or API tokens.</p>
	{:else}
		<p class="muted">Signed in as <strong>{auth.status.user?.username}</strong>.</p>

		<h2 class="section-label legend">API tokens</h2>
		<p class="muted intro">
			Scripts and cron jobs use a token to call the API, for example to push passive check results. A
			token is shown only once, right after you create it.
		</p>

		{#if created}
			<div class="plate created">
				<p>New token “{created.name}”. Copy it now; it can't be shown again.</p>
				<div class="secret">
					<code class="data">{created.token}</code>
					<button type="button" class="legend" onclick={copy}>{copied ? 'Copied' : 'Copy'}</button>
				</div>
				<pre class="data example">curl -X POST {location.origin}/api/v1/services/HOST/SERVICE/result \
  -H 'Authorization: Bearer {created.token}' \
  -H 'content-type: application/json' \
  -d '&#123;"exit_code": 0, "output": "BACKUP OK"&#125;'</pre>
			</div>
		{/if}

		<form class="new" onsubmit={create}>
			<input placeholder="What is it for, e.g. backup cron job" bind:value={name} required maxlength="100" aria-label="Token name" />
			<button type="submit" class="legend">Create token</button>
		</form>
		{#if error}<p class="error" role="alert">{error}</p>{/if}

		{#if tokens.length > 0}
			<table class="list">
				<thead>
					<tr>
						<th>Name</th>
						<th class="hide-narrow">Created</th>
						<th>Last used</th>
						<th></th>
					</tr>
				</thead>
				<tbody>
					{#each tokens as t (t.id)}
						<tr>
							<td>{t.name}</td>
							<td class="hide-narrow data">{timestamp(t.created)}</td>
							<td class="data">{t.last_used ? timestamp(t.last_used) : 'never'}</td>
							<td class="actions"><button type="button" class="legend" onclick={() => revoke(t)}>Revoke</button></td>
						</tr>
					{/each}
				</tbody>
			</table>
		{/if}
	{/if}
</main>

<style>
	h1 {
		font-size: 20px;
		margin-bottom: 8px;
	}

	.intro {
		margin: 0 0 16px;
		max-width: 60ch;
		font-size: 14px;
	}

	.created {
		padding: 16px;
		margin-bottom: 16px;
		border-color: var(--ok);
	}

	.created p {
		margin: 0 0 10px;
	}

	.secret {
		display: flex;
		gap: 10px;
		align-items: center;
		flex-wrap: wrap;
	}

	.secret code {
		padding: 6px 10px;
		background: var(--plate-sunk);
		border: 1px solid var(--rule);
		border-radius: var(--radius);
		overflow-wrap: anywhere;
	}

	.example {
		margin: 12px 0 0;
		padding: 10px 12px;
		font-size: 12px;
		background: var(--plate-sunk);
		border-radius: var(--radius);
		overflow-x: auto;
	}

	.new {
		display: flex;
		gap: 10px;
		margin-bottom: 16px;
		flex-wrap: wrap;
	}

	.new input {
		flex: 1;
		min-width: 220px;
		font: inherit;
		font-size: 14px;
		color: var(--ink);
		background: var(--plate);
		border: 1px solid var(--rule);
		border-radius: var(--radius);
		padding: 7px 10px;
	}

	button {
		font-size: 12px;
		padding: 7px 12px;
		color: var(--ink);
		background: var(--plate);
		border: 1px solid var(--rule);
		border-radius: var(--radius);
		cursor: pointer;
	}

	.new button {
		color: var(--plate);
		background: var(--ink);
		border-color: var(--ink);
	}

	.actions {
		text-align: right;
	}

	.error {
		color: var(--crit);
		font-size: 14px;
	}
</style>
