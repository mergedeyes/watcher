<script lang="ts">
	import { api, type User } from '#lib/api.ts';
	import { actions } from '#lib/actions.svelte.ts';
	import { auth } from '#lib/auth.svelte.ts';
	import { datetime } from '#lib/format.ts';

	let users = $state<User[]>([]);
	let error = $state<string | null>(null);
	let username = $state('');
	let password = $state('');
	let admin = $state(false);

	$effect(() => {
		if (auth.status?.required && auth.admin) void load();
	});

	async function load() {
		try {
			users = await api.users();
			error = null;
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	}

	/** Runs a change, reports it, reloads. */
	async function act(done: string, f: () => Promise<unknown>): Promise<boolean> {
		try {
			await f();
			actions.say(done);
			await load();
			return true;
		} catch (e) {
			actions.say(e instanceof Error ? e.message : String(e), true);
			return false;
		}
	}

	async function create(e: SubmitEvent) {
		e.preventDefault();
		const name = username.trim();
		if (await act(`User ${name} created`, () => api.createUser(name, password, admin))) {
			username = '';
			password = '';
			admin = false;
		}
	}

	function setPassword(u: User) {
		const pw = prompt(`New password for ${u.username} (min. 10 characters):`);
		if (pw) void act(`Password of ${u.username} set`, () => api.updateUser(u.username, { password: pw }));
	}

	function remove(u: User) {
		if (confirm(`Delete user ${u.username} with their tokens?`)) void act(`User ${u.username} deleted`, () => api.deleteUser(u.username));
	}

	let me = $derived(auth.status?.user?.username ?? '');
</script>

<div class="content">
	<div class="page-head"><h1>Users <span class="muted">{users.length}</span></h1></div>
	{#if !auth.status?.required}
		<p class="notice">Authentication disabled: no users.</p>
	{:else if !auth.admin}
		<p class="notice">Admins only.</p>
	{:else}
		{#if error}<p class="notice error">{error}</p>{/if}
		<section class="panel">
			<div class="panel-title">Add User</div>
			<form class="add" onsubmit={create}>
				<input placeholder="Username" bind:value={username} required maxlength="64" autocomplete="off" aria-label="Username" />
				<input type="password" placeholder="Password, min. 10" bind:value={password} required minlength="10" autocomplete="new-password" aria-label="Password" />
				<label><input type="checkbox" bind:checked={admin} /> Admin</label>
				<button type="submit" class="btn primary">Add</button>
			</form>
		</section>

		<section class="panel">
			<table class="grid">
				<thead>
					<tr>
						<th>Username</th>
						<th>Role</th>
						<th>Status</th>
						<th class="hide-narrow">Created</th>
						<th class="hide-narrow">Last Login</th>
						<th></th>
					</tr>
				</thead>
				<tbody>
					{#each users as u (u.id)}
						{@const self = u.username.toLowerCase() === me.toLowerCase()}
						<tr class:disabled={u.disabled}>
							<td class="nowrap">{u.username}{#if self}<span class="muted"> (you)</span>{/if}</td>
							<td>{u.admin ? 'admin' : 'user'}</td>
							<td>{#if u.disabled}<span class="tag off">DISABLED</span>{:else}active{/if}</td>
							<td class="nowrap hide-narrow">{datetime(u.created)}</td>
							<td class="nowrap hide-narrow">{u.last_login ? datetime(u.last_login) : 'never'}</td>
							<td class="nowrap actions">
								<button type="button" class="btn small" onclick={() => setPassword(u)}>Password</button>
								{#if !self}
									<button
										type="button"
										class="btn small"
										onclick={() => act(`${u.username}: ${u.admin ? 'user' : 'admin'}`, () => api.updateUser(u.username, { admin: !u.admin }))}
										>{u.admin ? 'Make User' : 'Make Admin'}</button
									>
									<button
										type="button"
										class="btn small"
										onclick={() =>
											act(`${u.username} ${u.disabled ? 'enabled' : 'disabled'}`, () => api.updateUser(u.username, { disabled: !u.disabled }))}
										>{u.disabled ? 'Enable' : 'Disable'}</button
									>
									<button type="button" class="btn small danger" onclick={() => remove(u)}>Delete</button>
								{/if}
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</section>
	{/if}
</div>

<style>
	.add {
		display: flex;
		flex-wrap: wrap;
		gap: 8px;
		align-items: center;
		padding: 12px;
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

	tr.disabled td {
		color: var(--muted);
	}

	.error {
		color: var(--crit);
	}
</style>
