<script lang="ts">
	import { onMount } from 'svelte';
	import { api, type ApiToken, type TotpSetup } from '#lib/api.ts';
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
		if (auth.status?.required) {
			load();
			loadTotp();
		}
	});

	// Two-factor authentication.
	let totp = $state<{ enabled: boolean; recovery_codes: number } | null>(null);
	let setup = $state<TotpSetup | null>(null);
	let totpCode = $state('');
	let recoveryCodes = $state<string[] | null>(null);
	let recoveryCopied = $state(false);
	let totpError = $state<string | null>(null);

	async function loadTotp() {
		try {
			totp = await api.totp();
		} catch (e) {
			totpError = e instanceof Error ? e.message : String(e);
		}
	}

	/** Runs a two-factor step; errors show in the panel. */
	async function totpStep(f: () => Promise<void>) {
		totpError = null;
		try {
			await f();
		} catch (e) {
			totpError = e instanceof Error ? e.message : String(e);
		}
		totpCode = '';
	}

	const startSetup = () =>
		totpStep(async () => {
			recoveryCodes = null;
			setup = await api.totpSetup();
		});

	function enable(event: SubmitEvent) {
		event.preventDefault();
		if (!setup) return;
		const secret = setup.secret;
		void totpStep(async () => {
			showCodes((await api.totpEnable(secret, totpCode.trim())).recovery_codes);
			setup = null;
			await Promise.all([loadTotp(), auth.refresh()]);
		});
	}

	const newRecoveryCodes = () =>
		totpStep(async () => {
			showCodes((await api.totpRecovery(totpCode.trim())).recovery_codes);
			await loadTotp();
		});

	const disable = () =>
		totpStep(async () => {
			await api.totpDisable(totpCode.trim());
			recoveryCodes = null;
			await Promise.all([loadTotp(), auth.refresh()]);
		});

	function showCodes(codes: string[]) {
		recoveryCodes = codes;
		recoveryCopied = false;
	}

	async function copyCodes() {
		if (!recoveryCodes) return;
		await navigator.clipboard.writeText(recoveryCodes.join('\n') + '\n');
		recoveryCopied = true;
	}

	/** The secret in groups of four, as apps show it. */
	const grouped = (secret: string) => secret.replace(/(.{4})(?=.)/g, '$1 ');

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

	let password = $state('');
	let repeat = $state('');
	let passwordError = $state<string | null>(null);

	async function changePassword(event: SubmitEvent) {
		event.preventDefault();
		passwordError = null;
		if (password !== repeat) {
			passwordError = 'Passwords differ';
			return;
		}
		const me = auth.status?.user?.username;
		if (!me) return;
		try {
			await api.updateUser(me, { password });
			// All sessions ended: log in again.
			auth.expire();
		} catch (e) {
			passwordError = e instanceof Error ? e.message : String(e);
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
			<div class="panel-title">Password</div>
			<form class="body new" onsubmit={changePassword}>
				<input type="password" placeholder="New password" bind:value={password} required minlength="10" autocomplete="new-password" aria-label="New password" />
				<input type="password" placeholder="Repeat" bind:value={repeat} required minlength="10" autocomplete="new-password" aria-label="Repeat password" />
				<button type="submit" class="btn primary">Change</button>
				{#if passwordError}<span class="error" role="alert">{passwordError}</span>{/if}
				<span class="muted">ends all sessions, this one too</span>
			</form>
		</section>

		<section class="panel">
			<div class="panel-title">Two-Factor Authentication</div>
			<div class="body">
				{#if recoveryCodes}
					<div class="created">
						<div>Recovery codes (shown once; each works once, instead of an app code):</div>
						<div class="codes mono">
							{#each recoveryCodes as c (c)}<code>{c}</code>{/each}
						</div>
						<button type="button" class="btn" onclick={copyCodes}>{recoveryCopied ? 'Copied' : 'Copy'}</button>
					</div>
				{/if}
				{#if totp === null}
					{#if totpError}<p class="error" role="alert">{totpError}</p>{/if}
				{:else if setup}
					<div class="setup">
						<img class="qr" src={`data:image/svg+xml;utf8,${encodeURIComponent(setup.qr)}`} alt="QR code for the authenticator app" />
						<div class="steps">
							<p>Scan with an authenticator app, or enter the key:</p>
							<code class="key">{grouped(setup.secret)}</code>
							<form class="new" onsubmit={enable}>
								<input
									placeholder="Code from the app"
									bind:value={totpCode}
									required
									autocomplete="one-time-code"
									inputmode="numeric"
									aria-label="Code from the app"
								/>
								<button type="submit" class="btn primary">Enable</button>
								<button type="button" class="btn" onclick={() => (setup = null)}>Cancel</button>
							</form>
						</div>
					</div>
				{:else if totp.enabled}
					<p>On <span class="muted">· {totp.recovery_codes} recovery codes left</span></p>
					<form class="new" onsubmit={(e) => e.preventDefault()}>
						<input placeholder="Code or recovery code" bind:value={totpCode} required autocomplete="one-time-code" aria-label="Code or recovery code" />
						<button type="button" class="btn" disabled={!totpCode.trim()} onclick={newRecoveryCodes}>New Recovery Codes</button>
						<button type="button" class="btn danger" disabled={!totpCode.trim()} onclick={disable}>Turn Off</button>
					</form>
				{:else}
					<div class="new">
						<span>Off</span>
						<button type="button" class="btn primary" onclick={startSetup}>Set Up</button>
					</div>
				{/if}
				{#if totp !== null && totpError}<p class="error" role="alert">{totpError}</p>{/if}
			</div>
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

	.setup {
		display: flex;
		gap: 16px;
		flex-wrap: wrap;
		align-items: flex-start;
	}

	.setup p {
		margin: 0 0 8px;
	}

	.qr {
		width: 200px;
		height: 200px;
		image-rendering: pixelated;
		border-radius: var(--radius);
	}

	.steps {
		flex: 1;
		min-width: 240px;
	}

	.key {
		display: inline-block;
		margin-bottom: 10px;
		padding: 4px 8px;
		background: var(--panel-alt);
		border: 1px solid var(--line);
		border-radius: var(--radius);
		overflow-wrap: anywhere;
	}

	.codes {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(120px, 1fr));
		gap: 4px 12px;
		margin: 8px 0;
	}

	.btn.danger {
		color: var(--crit);
		border-color: var(--crit);
	}

	.error {
		color: var(--crit);
	}
</style>
