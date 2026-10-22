<!--
	First-run setup: unlocked with the token from the server log, then the
	same answers as `watcher init`, a summary of the files, and a handover
	to the normal server.
-->
<script lang="ts">
	import { onMount } from 'svelte';

	interface Defaults {
		dir: string;
		hostname: string;
		listen: string;
		plugins: string | null;
		existing: string[];
		min_password: number;
	}

	interface PlannedFile {
		path: string;
		contents: string;
		exists: boolean;
	}

	const STEPS = ['Unlock', 'Instance', 'Database', 'Web Access', 'Admin', 'Notifications', 'Monitoring', 'Summary'] as const;

	let step = $state(0);
	let token = $state('');
	let defaults = $state<Defaults | null>(null);
	let error = $state<string | null>(null);
	let busy = $state(false);

	// Answers
	let role = $state<'standalone' | 'hub'>('standalone');
	let instanceName = $state('');
	let databaseUrl = $state('');
	let dbResult = $state<string | null>(null);
	let listen = $state('127.0.0.1:8080');
	let publicUrl = $state('');
	let tlsMode = $state<'none' | 'self' | 'files'>('none');
	let cert = $state('');
	let key = $state('');
	let adminName = $state('admin');
	let adminPassword = $state('');
	let adminConfirm = $state('');
	let alertEmail = $state('');
	let alertWebhook = $state('');
	let smtpOn = $state(false);
	let smtp = $state({ host: '', port: '', tls: 'starttls', username: '', password: '', from: '' });
	let testResult = $state<string | null>(null);
	let monitorLocalhost = $state(true);
	let files = $state<PlannedFile[]>([]);
	let done = $state<{ url: string } | null>(null);

	onMount(() => {
		try {
			token = sessionStorage.getItem('watcher-setup-token') ?? '';
		} catch {
			// storage blocked
		}
		if (token) unlock();
	});

	async function call<T>(method: string, path: string, body?: unknown): Promise<T> {
		const res = await fetch(`/api/v1/setup${path}`, {
			method,
			headers: { 'x-setup-token': token.trim(), ...(body === undefined ? {} : { 'content-type': 'application/json' }) },
			body: body === undefined ? undefined : JSON.stringify(body)
		});
		const json = await res.json().catch(() => ({}));
		if (!res.ok) throw new Error(json.error ?? `${res.status} ${res.statusText}`);
		return json as T;
	}

	async function run(action: () => Promise<void>) {
		error = null;
		busy = true;
		try {
			await action();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			busy = false;
		}
	}

	function unlock(event?: SubmitEvent) {
		event?.preventDefault();
		return run(async () => {
			const res = await fetch('/api/v1/setup/unlock', {
				method: 'POST',
				headers: { 'content-type': 'application/json' },
				body: JSON.stringify({ token: token.trim() })
			});
			if (!res.ok) throw new Error('Wrong setup token.');
			try {
				sessionStorage.setItem('watcher-setup-token', token.trim());
			} catch {
				// storage blocked
			}
			defaults = await call<Defaults>('GET', '');
			listen = defaults.listen;
			instanceName ||= defaults.hostname;
			step = 1;
		});
	}

	function testDatabase() {
		return run(async () => {
			dbResult = null;
			const r = await call<{ backend: string; users: number }>('POST', '/database', { url: databaseUrl.trim() });
			dbResult = `${r.backend}; schema ready${r.users ? `; ${r.users} user(s) exist` : ''}`;
		});
	}

	function list(text: string) {
		return text
			.split(',')
			.map((s) => s.trim())
			.filter(Boolean);
	}

	function smtpAnswer() {
		if (!smtpOn) return null;
		return {
			host: smtp.host.trim(),
			port: smtp.port.trim() ? Number(smtp.port) : null,
			tls: smtp.tls,
			username: smtp.username.trim() || null,
			password: smtp.password || null,
			from: smtp.from.trim()
		};
	}

	function sendTest() {
		return run(async () => {
			testResult = null;
			const r = await call<{ results: { channel: string; error: string | null }[] }>('POST', '/notify-test', {
				smtp: smtpAnswer(),
				email: list(alertEmail),
				webhook: alertWebhook.trim() || null
			});
			testResult = r.results.length
				? r.results.map((x) => `${x.channel}: ${x.error ?? 'sent'}`).join('; ')
				: 'No email or webhook to test.';
		});
	}

	let database = $derived(databaseUrl.trim() !== '');

	function answers() {
		return {
			role,
			instance_name: instanceName.trim() || null,
			listen: listen.trim(),
			public_url: publicUrl.trim() || null,
			tls:
				tlsMode === 'self'
					? { self_signed: true }
					: tlsMode === 'files'
						? { cert: cert.trim(), key: key.trim() }
						: null,
			database_url: database ? databaseUrl.trim() : null,
			admin: database ? { username: adminName.trim(), password: adminPassword } : null,
			smtp: smtpAnswer(),
			alert_email: list(alertEmail),
			alert_webhook: alertWebhook.trim() || null,
			monitor_localhost: monitorLocalhost,
			test_notification: false
		};
	}

	/** Checks for a step before moving on; returns an error or null. */
	function check(s: number): string | null {
		switch (STEPS[s]) {
			case 'Admin':
				if (!database) return null;
				if (!adminName.trim()) return 'Username required.';
				if (adminPassword.length < (defaults?.min_password ?? 10))
					return `Password: min. ${defaults?.min_password ?? 10} characters.`;
				if (adminPassword !== adminConfirm) return 'Passwords differ.';
				return null;
			case 'Web Access':
				if (tlsMode === 'files' && (!cert.trim() || !key.trim())) return 'Certificate and key paths required.';
				return null;
			default:
				return null;
		}
	}

	function next() {
		error = check(step);
		if (error) return;
		step++;
		if (STEPS[step] === 'Summary') {
			files = [];
			run(async () => {
				files = (await call<{ files: PlannedFile[] }>('POST', '/plan', answers())).files;
			});
		}
	}

	function back() {
		error = null;
		step--;
	}

	function apply() {
		return run(async () => {
			const r = await call<{ listen: string; scheme: string }>('POST', '/apply', answers());
			try {
				sessionStorage.removeItem('watcher-setup-token');
			} catch {
				// storage blocked
			}
			const [host, port] = splitAddr(r.listen);
			const sameHost = ['0.0.0.0', '[::]', '::'].includes(host) ? location.hostname : host;
			const url = `${r.scheme}://${sameHost.includes(':') && !sameHost.startsWith('[') ? `[${sameHost}]` : sameHost}:${port}/`;
			done = { url };
			// The server restarts with the new config; continue there.
			if (url.startsWith(location.origin)) waitFor(url);
		});
	}

	function splitAddr(addr: string): [string, string] {
		const i = addr.lastIndexOf(':');
		return [addr.slice(0, i), addr.slice(i + 1)];
	}

	async function waitFor(url: string) {
		for (let i = 0; i < 60; i++) {
			await new Promise((r) => setTimeout(r, 1000));
			try {
				const h = await (await fetch('/api/v1/health')).json();
				if (h.status === 'ok') {
					location.href = url;
					return;
				}
			} catch {
				// restarting
			}
		}
	}
</script>

<svelte:head><title>Watcher Setup</title></svelte:head>

<div class="setup">
	<header>
		<h1>Watcher Setup</h1>
		{#if defaults}<span class="muted mono">{defaults.dir}</span>{/if}
	</header>

	<ol class="steps">
		{#each STEPS as s, i (s)}
			<li class:current={i === step} class:done={i < step}>{s}</li>
		{/each}
	</ol>

	{#if done}
		<section class="panel">
			<div class="panel-title">Done</div>
			<div class="body">
				<p>Configuration written; Watcher is starting.</p>
				<p>Continue at <a href={done.url}>{done.url}</a>{database ? ' and log in as ' + adminName : ''}.</p>
			</div>
		</section>
	{:else}
		<form
			class="panel"
			onsubmit={(e) => {
				e.preventDefault();
				if (step === 0) unlock();
				else if (STEPS[step] === 'Summary') apply();
				else next();
			}}
		>
			<div class="panel-title">{STEPS[step]}</div>
			<div class="body">
				{#if step === 0}
					<p class="muted">Setup token: printed in the server log (<code>setup token: …</code>).</p>
					<div class="fields">
						<label for="token">Token</label>
						<input id="token" class="mono" autocomplete="off" bind:value={token} required />
					</div>
				{:else if STEPS[step] === 'Instance'}
					{#if defaults && defaults.existing.length > 0}
						<p class="warn">Exists in {defaults.dir}: {defaults.existing.join(', ')}. Files are overwritten.</p>
					{/if}
					<div class="fields">
						<label for="role">Role</label>
						<select id="role" bind:value={role}>
							<option value="standalone">standalone (hub + local checks)</option>
							<option value="hub">hub</option>
						</select>
						<label for="name">Instance Name</label>
						<div>
							<input id="name" bind:value={instanceName} />
							<div class="help">sidebar, page title, mail subject</div>
						</div>
					</div>
				{:else if STEPS[step] === 'Database'}
					<div class="fields">
						<label for="db">PostgreSQL URL</label>
						<div>
							<input id="db" class="mono" placeholder="postgres://watcher:secret@localhost/watcher" bind:value={databaseUrl} />
							<div class="help">history, graphs, user accounts; empty: live state only, loopback only</div>
						</div>
						<span></span>
						<div>
							<button type="button" class="btn" onclick={testDatabase} disabled={!database || busy}>Test Connection</button>
							{#if dbResult}<span class="ok">{dbResult}</span>{/if}
						</div>
					</div>
				{:else if STEPS[step] === 'Web Access'}
					<div class="fields">
						<label for="listen">Listen</label>
						<div>
							<input id="listen" class="mono" bind:value={listen} />
							<div class="help">0.0.0.0:8080 for remote access (needs a database for logins)</div>
						</div>
						<label for="public">Public URL</label>
						<div>
							<input id="public" class="mono" placeholder="https://watcher.example.com" bind:value={publicUrl} />
							<div class="help">links in notifications</div>
						</div>
						<label for="tls">TLS</label>
						<select id="tls" bind:value={tlsMode}>
							<option value="none">none (HTTP, or a TLS reverse proxy)</option>
							<option value="self">self-signed certificate</option>
							<option value="files">certificate files</option>
						</select>
						{#if tlsMode === 'files'}
							<label for="cert">Certificate</label>
							<input id="cert" class="mono" placeholder="/etc/ssl/watcher/fullchain.pem" bind:value={cert} />
							<label for="key">Key</label>
							<input id="key" class="mono" placeholder="/etc/ssl/watcher/privkey.pem" bind:value={key} />
						{/if}
					</div>
				{:else if STEPS[step] === 'Admin'}
					{#if !database}
						<p class="muted">No database: no user accounts. The web UI is open on loopback only.</p>
					{:else}
						<div class="fields">
							<label for="user">Username</label>
							<input id="user" autocomplete="username" bind:value={adminName} />
							<label for="pw">Password</label>
							<div>
								<input id="pw" type="password" autocomplete="new-password" bind:value={adminPassword} />
								<div class="help">min. {defaults?.min_password ?? 10} characters</div>
							</div>
							<label for="pw2">Confirm</label>
							<input id="pw2" type="password" autocomplete="new-password" bind:value={adminConfirm} />
						</div>
					{/if}
				{:else if STEPS[step] === 'Notifications'}
					<div class="fields">
						<label for="mail">Alert Email</label>
						<div>
							<input id="mail" placeholder="ops@example.com, oncall@example.com" bind:value={alertEmail} />
							<div class="help">contact "admins"; comma-separated</div>
						</div>
						<label for="hook">Alert Webhook</label>
						<input id="hook" class="mono" placeholder="https://hooks.example.com/watcher" bind:value={alertWebhook} />
						<label for="smtp-on">SMTP</label>
						<input id="smtp-on" type="checkbox" bind:checked={smtpOn} />
						{#if smtpOn}
							<label for="smtp-host">Host</label>
							<input id="smtp-host" bind:value={smtp.host} required />
							<label for="smtp-tls">Encryption</label>
							<select id="smtp-tls" bind:value={smtp.tls}>
								<option value="starttls">STARTTLS (587)</option>
								<option value="tls">TLS (465)</option>
								<option value="none">none (25)</option>
							</select>
							<label for="smtp-port">Port</label>
							<input id="smtp-port" inputmode="numeric" placeholder="default for encryption" bind:value={smtp.port} />
							<label for="smtp-user">Username</label>
							<input id="smtp-user" autocomplete="off" bind:value={smtp.username} />
							<label for="smtp-pw">Password</label>
							<input id="smtp-pw" type="password" autocomplete="off" bind:value={smtp.password} />
							<label for="smtp-from">From</label>
							<input id="smtp-from" placeholder="Watcher <watcher@example.com>" bind:value={smtp.from} required />
						{/if}
						<span></span>
						<div>
							<button type="button" class="btn" onclick={sendTest} disabled={busy || (!alertEmail.trim() && !alertWebhook.trim())}>Send Test</button>
							{#if testResult}<span class="muted">{testResult}</span>{/if}
						</div>
					</div>
				{:else if STEPS[step] === 'Monitoring'}
					<div class="fields">
						<label for="local">This Machine</label>
						<div>
							<input id="local" type="checkbox" bind:checked={monitorLocalhost} />
							<span class="help">
								disk, load, swap{defaults?.hostname ? ` of ${defaults.hostname}` : ''};
								{defaults?.plugins ? `plugins in ${defaults.plugins}` : 'no monitoring plugins found: shell checks'}
							</span>
						</div>
					</div>
				{:else if STEPS[step] === 'Summary'}
					{#if files.length === 0 && !error}<p class="muted">Checking…</p>{/if}
					{#each files as f (f.path)}
						<div class="file">
							<div class="mono path">{f.path}{f.exists ? ' (overwrite)' : ''}</div>
							<pre class="mono">{f.contents}</pre>
						</div>
					{/each}
					{#if database}<p>Create user: <strong>{adminName}</strong> (admin)</p>{/if}
				{/if}
				{#if error}<p class="error" role="alert">{error}</p>{/if}
			</div>
			<div class="foot">
				{#if step > 1}<button type="button" class="btn" onclick={back} disabled={busy}>Back</button>{/if}
				<span class="spacer"></span>
				<button type="submit" class="btn primary" disabled={busy || (STEPS[step] === 'Summary' && files.length === 0)}>
					{step === 0 ? 'Unlock' : STEPS[step] === 'Summary' ? 'Write & Start' : 'Next'}
				</button>
			</div>
		</form>
	{/if}
</div>

<style>
	.setup {
		max-width: 760px;
		margin: 0 auto;
		padding: 32px 16px 48px;
	}

	header {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: 12px;
		flex-wrap: wrap;
		margin-bottom: 14px;
	}

	.steps {
		display: flex;
		flex-wrap: wrap;
		gap: 4px;
		margin: 0 0 14px;
		padding: 0;
		list-style: none;
		counter-reset: step;
	}

	.steps li {
		counter-increment: step;
		padding: 3px 8px;
		border: 1px solid var(--line);
		border-radius: 3px;
		font-size: 12px;
		color: var(--muted);
		background: var(--panel);
	}

	.steps li::before {
		content: counter(step) ' ';
		font-weight: 700;
	}

	.steps li.done {
		color: var(--ok);
		border-color: var(--ok);
	}

	.steps li.current {
		color: #fff;
		background: var(--link);
		border-color: var(--link);
	}

	.body {
		padding: 14px;
	}

	.body > p {
		margin: 0 0 10px;
	}

	.fields {
		display: grid;
		grid-template-columns: 150px minmax(0, 1fr);
		gap: 8px 14px;
		align-items: center;
	}

	.fields input:not([type='checkbox']),
	.fields select {
		width: 100%;
	}

	label {
		color: var(--muted);
		font-weight: 500;
	}

	.help {
		font-size: 11px;
		color: var(--muted);
		margin-top: 2px;
	}

	.ok {
		margin-left: 8px;
		color: var(--ok);
		font-weight: 600;
	}

	.warn {
		color: var(--unknown);
	}

	.error {
		margin: 12px 0 0;
		color: var(--crit);
		white-space: pre-wrap;
	}

	.file + .file {
		margin-top: 10px;
	}

	.path {
		font-weight: 600;
		margin-bottom: 4px;
	}

	pre {
		margin: 0;
		padding: 8px 10px;
		background: var(--panel-alt);
		border: 1px solid var(--line);
		border-radius: var(--radius);
		max-height: 240px;
		overflow: auto;
	}

	.foot {
		display: flex;
		gap: 8px;
		padding: 10px 14px;
		border-top: 1px solid var(--line);
	}

	.spacer {
		flex: 1;
	}

	@media (max-width: 720px) {
		.fields {
			grid-template-columns: 1fr;
			gap: 2px;
		}
		label {
			margin-top: 8px;
		}
	}
</style>
