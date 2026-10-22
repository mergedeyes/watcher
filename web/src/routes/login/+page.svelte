<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api, ApiError } from '#lib/api.ts';
	import { auth } from '#lib/auth.svelte.ts';

	let username = $state('');
	let password = $state('');
	let error = $state<string | null>(null);
	let busy = $state(false);

	/** Only same-site paths, so the login can't redirect elsewhere. */
	let next = $derived.by(() => {
		const n = page.url.searchParams.get('next') ?? '/';
		return n.startsWith('/') && !n.startsWith('//') ? n : '/';
	});

	// Already signed in, or no login needed: go on.
	$effect(() => {
		if (auth.allowed) goto(next, { replaceState: true });
	});

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		busy = true;
		error = null;
		try {
			await api.login(username, password);
			await auth.refresh();
		} catch (e) {
			error =
				e instanceof ApiError && e.status === 401
					? 'Invalid credentials.'
					: e instanceof Error
						? e.message
						: String(e);
			password = '';
		} finally {
			busy = false;
		}
	}
</script>

<div class="wrap">
	<form class="panel box" onsubmit={submit}>
		<div class="brand">Watcher</div>

		{#if auth.status && !auth.status.users_exist}
			<p class="notice">No users. Create one: <code>watcher user add &lt;name&gt; --admin</code></p>
		{/if}

		<label>
			Username
			<input name="username" autocomplete="username" required bind:value={username} />
		</label>
		<label>
			Password
			<input name="password" type="password" autocomplete="current-password" required bind:value={password} />
		</label>

		{#if error}<p class="error" role="alert">{error}</p>{/if}

		<button type="submit" class="btn primary" disabled={busy}>{busy ? 'Signing in…' : 'Sign in'}</button>
	</form>
</div>

<style>
	.wrap {
		min-height: 100vh;
		display: grid;
		place-items: center;
		padding: 16px;
		background: var(--side-bg);
	}

	.box {
		width: 100%;
		max-width: 340px;
		padding: 24px;
		display: flex;
		flex-direction: column;
		gap: 12px;
	}

	.brand {
		font-size: 18px;
		font-weight: 700;
		letter-spacing: 0.04em;
		margin-bottom: 4px;
	}

	label {
		display: flex;
		flex-direction: column;
		gap: 4px;
		font-weight: 600;
		font-size: 12px;
		color: var(--muted);
	}

	label input {
		font-weight: 400;
		font-size: 13px;
	}

	button {
		margin-top: 4px;
		padding: 7px;
	}

	.error {
		margin: 0;
		color: var(--crit);
	}
</style>
