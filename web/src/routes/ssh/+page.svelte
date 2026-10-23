<script lang="ts">
	import { api, type HostKey, type NewSecret, type Secret } from '#lib/api.ts';
	import { actions } from '#lib/actions.svelte.ts';
	import { auth } from '#lib/auth.svelte.ts';
	import { datetime } from '#lib/format.ts';

	let secrets = $state<Secret[]>([]);
	let hostKeys = $state<HostKey[]>([]);
	let error = $state<string | null>(null);

	let kind = $state<NewSecret['kind']>('generate');
	let name = $state('');
	let privateKey = $state('');
	let passphrase = $state('');
	let password = $state('');
	let replace = $state(false);
	/** The key just generated, to copy into authorized_keys. */
	let created = $state<Secret | null>(null);
	let copied = $state(false);

	$effect(() => {
		if (auth.admin) void load();
	});

	async function load() {
		try {
			[secrets, hostKeys] = await Promise.all([api.secrets(), api.hostKeys()]);
			error = null;
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	}

	/** Runs a change, reports it, reloads. */
	async function act<T>(done: string, f: () => Promise<T>): Promise<T | undefined> {
		try {
			const result = await f();
			actions.say(done);
			await load();
			return result;
		} catch (e) {
			actions.say(e instanceof Error ? e.message : String(e), true);
			return undefined;
		}
	}

	async function add(e: SubmitEvent) {
		e.preventDefault();
		const secret: NewSecret = { name: name.trim(), kind, replace };
		if (kind === 'key') {
			secret.private_key = privateKey;
			if (passphrase) secret.passphrase = passphrase;
		}
		if (kind === 'password') secret.password = password;
		const result = await act(`Secret ${secret.name} stored`, () => api.createSecret(secret));
		if (!result) return;
		created = kind === 'generate' ? result : null;
		copied = false;
		name = privateKey = passphrase = password = '';
		replace = false;
	}

	async function copy(text: string) {
		await navigator.clipboard.writeText(text);
		copied = true;
	}

	function remove(s: Secret) {
		const used = s.hosts.length ? ` Used by ${s.hosts.length} host(s).` : '';
		if (confirm(`Delete secret ${s.name}?${used}`)) void act(`Secret ${s.name} deleted`, () => api.deleteSecret(s.name));
	}

	function target(k: HostKey) {
		return k.port === 22 ? k.address : `${k.address}:${k.port}`;
	}

	function accept(k: HostKey) {
		if (confirm(`Trust the new key of ${target(k)}?\n${k.offered_fingerprint}`))
			void act(`New key of ${target(k)} pinned`, () => api.acceptHostKey(k.address, k.port));
	}

	function forget(k: HostKey) {
		if (confirm(`Forget the key of ${target(k)}? The next one is pinned.`))
			void act(`Key of ${target(k)} forgotten`, () => api.forgetHostKey(k.address, k.port));
	}

	let changed = $derived(hostKeys.filter((k) => k.offered).length);
</script>

<div class="content">
	<div class="page-head"><h1>SSH</h1></div>
	{#if !auth.admin}
		<p class="notice">Admins only.</p>
	{:else}
		{#if error}<p class="notice error">{error}</p>{/if}

		<section class="panel">
			<div class="panel-title">Secrets <span class="muted">{secrets.length}</span></div>
			<div class="body">
				{#if created?.public_key}
					<div class="created">
						<div>Public key of "{created.name}", for <code>~/.ssh/authorized_keys</code>:</div>
						<div class="secret">
							<code>{created.public_key}</code>
							<button type="button" class="btn" onclick={() => copy(created?.public_key ?? '')}>{copied ? 'Copied' : 'Copy'}</button>
						</div>
					</div>
				{/if}
				<form class="new" onsubmit={add}>
					<select bind:value={kind} aria-label="Kind">
						<option value="generate">New Key</option>
						<option value="key">Import Key</option>
						<option value="password">Password</option>
					</select>
					<input placeholder="Name, e.g. monitor" bind:value={name} required maxlength="64" autocomplete="off" aria-label="Name" />
					{#if kind === 'password'}
						<input type="password" placeholder="Password" bind:value={password} required autocomplete="new-password" aria-label="Password" />
					{:else if kind === 'key'}
						<input type="password" placeholder="Passphrase, if encrypted" bind:value={passphrase} autocomplete="off" aria-label="Passphrase" />
					{/if}
					<label class="nowrap"><input type="checkbox" bind:checked={replace} /> Replace</label>
					<button type="submit" class="btn primary">{kind === 'generate' ? 'Generate' : 'Store'}</button>
					{#if kind === 'key'}
						<textarea
							class="mono"
							rows="6"
							placeholder="-----BEGIN OPENSSH PRIVATE KEY----- (ed25519 or ECDSA)"
							bind:value={privateKey}
							required
							spellcheck="false"
							aria-label="Private key"
						></textarea>
					{/if}
				</form>
				<p class="muted hint">Hosts: <code>ssh = &#123; user = "monitor", secret = "NAME" &#125;</code>; services: <code>run_on = "host"</code></p>
			</div>
			{#if secrets.length > 0}
				<table class="grid">
					<thead>
						<tr>
							<th>Name</th>
							<th>Kind</th>
							<th class="hide-narrow">Fingerprint</th>
							<th>Hosts</th>
							<th class="hide-narrow">Updated</th>
							<th></th>
						</tr>
					</thead>
					<tbody>
						{#each secrets as s (s.name)}
							<tr>
								<td class="nowrap">{s.name}</td>
								<td>{s.kind}</td>
								<td class="mono hide-narrow">{s.fingerprint ?? ''}</td>
								<td>{s.hosts.length ? s.hosts.join(', ') : '—'}</td>
								<td class="nowrap hide-narrow">{datetime(s.updated)}</td>
								<td class="nowrap actions">
									{#if s.public_key}
										<button type="button" class="btn small" onclick={() => copy(s.public_key ?? '').then(() => actions.say('Public key copied'))}
											>Copy Public Key</button
										>
									{/if}
									<button type="button" class="btn small danger" onclick={() => remove(s)}>Delete</button>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			{/if}
		</section>

		<section class="panel">
			<div class="panel-title">
				Host Keys
				<span>{#if changed}<span class="tag changed">{changed} CHANGED</span>{/if} <span class="muted">{hostKeys.length}</span></span>
			</div>
			{#if hostKeys.length === 0}
				<p class="body muted">None yet: pinned on the first connection.</p>
			{:else}
				<table class="grid">
					<thead>
						<tr>
							<th>Address</th>
							<th>Fingerprint</th>
							<th>Hosts</th>
							<th class="hide-narrow">Pinned</th>
							<th></th>
						</tr>
					</thead>
					<tbody>
						{#each hostKeys as k (`${k.address}:${k.port}`)}
							<tr class:offered={k.offered}>
								<td class="nowrap">{target(k)}</td>
								<td class="mono">
									{k.fingerprint ?? ''}
									{#if k.offered}
										<div class="new-key">
											<span class="tag changed">CHANGED</span>
											{k.offered_fingerprint}
											<span class="muted">{k.offered_at ? datetime(k.offered_at) : ''}</span>
										</div>
									{/if}
								</td>
								<td>{k.hosts.length ? k.hosts.join(', ') : '—'}</td>
								<td class="nowrap hide-narrow">{datetime(k.pinned)}</td>
								<td class="nowrap actions">
									{#if k.offered}<button type="button" class="btn small" onclick={() => accept(k)}>Accept New Key</button>{/if}
									<button type="button" class="btn small danger" onclick={() => forget(k)}>Forget</button>
								</td>
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

	.new input:not([type='checkbox']) {
		flex: 1;
		min-width: 180px;
	}

	.new textarea {
		flex-basis: 100%;
		font-size: 12px;
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

	.secret {
		display: flex;
		gap: 8px;
		align-items: center;
		flex-wrap: wrap;
		margin: 6px 0 0;
	}

	.secret code {
		padding: 4px 8px;
		background: var(--panel-alt);
		border: 1px solid var(--line);
		border-radius: var(--radius);
		overflow-wrap: anywhere;
	}

	.mono {
		font-size: 12px;
		overflow-wrap: anywhere;
	}

	.new-key {
		margin-top: 2px;
	}

	.tag.changed {
		margin-left: 4px;
		font-weight: 600;
		color: var(--crit);
		border-color: var(--crit);
	}

	tr.offered td {
		background: var(--tint-crit);
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

	.error {
		color: var(--crit);
	}
</style>
