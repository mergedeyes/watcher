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
					? 'Wrong username or password.'
					: e instanceof Error
						? e.message
						: String(e);
			password = '';
		} finally {
			busy = false;
		}
	}
</script>

<main class="page">
	<form class="plate box" onsubmit={submit}>
		<h1 class="legend">Sign in</h1>

		{#if auth.status && !auth.status.users_exist}
			<p class="notice">
				No accounts exist yet. Create the first one on the server:<br />
				<code>watcher user add &lt;name&gt; --admin</code>
			</p>
		{/if}

		<label>
			<span class="legend">Username</span>
			<input name="username" autocomplete="username" required bind:value={username} />
		</label>
		<label>
			<span class="legend">Password</span>
			<input name="password" type="password" autocomplete="current-password" required bind:value={password} />
		</label>

		{#if error}<p class="error" role="alert">{error}</p>{/if}

		<button type="submit" class="legend" disabled={busy}>{busy ? 'Signing in…' : 'Sign in'}</button>
	</form>
</main>

<style>
	.box {
		max-width: 380px;
		margin: 48px auto 0;
		padding: 28px;
		display: flex;
		flex-direction: column;
		gap: 16px;
	}

	h1 {
		font-size: 20px;
	}

	label {
		display: flex;
		flex-direction: column;
		gap: 6px;
	}

	label .legend {
		font-size: 11px;
		color: var(--ink-2);
	}

	input {
		font: inherit;
		color: var(--ink);
		background: var(--plate-sunk);
		border: 1px solid var(--rule);
		border-radius: var(--radius);
		padding: 8px 10px;
	}

	button {
		margin-top: 4px;
		font-size: 13px;
		padding: 10px;
		color: var(--plate);
		background: var(--ink);
		border: 0;
		border-radius: var(--radius);
		cursor: pointer;
	}

	button:disabled {
		opacity: 0.6;
		cursor: default;
	}

	.error {
		margin: 0;
		color: var(--crit);
		font-size: 14px;
	}

	.notice code {
		font-size: 13px;
	}
</style>
