<script lang="ts">
	import '../app.css';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import { api, setUnauthorizedHandler } from '#lib/api.ts';
	import { auth } from '#lib/auth.svelte.ts';
	import { live } from '#lib/live.svelte.ts';

	let { children } = $props();
	let unreachable = $state<string | null>(null);
	let menuOpen = $state(false);

	onMount(async () => {
		setUnauthorizedHandler(() => auth.expire());
		try {
			await auth.refresh();
		} catch (e) {
			unreachable = e instanceof Error ? e.message : String(e);
		}
	});

	// Load live data once allowed; otherwise stop it and go to the login page.
	$effect(() => {
		if (!auth.status) return;
		const onLogin = page.url.pathname === '/login';
		if (auth.allowed) {
			live.start();
		} else {
			live.stop();
			if (!onLogin) {
				const next = page.url.pathname + page.url.search;
				goto(`/login?next=${encodeURIComponent(next)}`, { replaceState: true });
			}
		}
	});

	// Close the mobile menu on navigation.
	$effect(() => {
		void page.url.pathname;
		menuOpen = false;
	});

	async function signOut() {
		await api.logout();
		auth.expire();
	}

	let problems = $derived(
		live.services.filter((s) => s.state !== null && s.state !== 'ok').length +
			live.hosts.filter((h) => h.state === 'down').length
	);
	let bare = $derived(page.url.pathname === '/login' || !auth.allowed);

	const NAV = [
		{
			title: 'General',
			items: [{ href: '/', label: 'Tactical Overview' }]
		},
		{
			title: 'Current Status',
			items: [
				{ href: '/hosts', label: 'Hosts' },
				{ href: '/services', label: 'Services' },
				{ href: '/problems', label: 'Problems' }
			]
		}
	];

	// Config editing is for admins; without login everyone may.
	let admin = $derived(!auth.status?.required || auth.status.user?.admin === true);

	const CONFIG_NAV = [
		{ href: '/config/hosts', label: 'Hosts' },
		{ href: '/config/checks', label: 'Checks' },
		{ href: '/config/contacts', label: 'Contacts' },
		{ href: '/config/templates', label: 'Templates' },
		{ href: '/config/files', label: 'Files' }
	];

	function active(href: string) {
		const path = page.url.pathname;
		return href === '/' ? path === '/' : path === href || path.startsWith(`${href}/`);
	}

	const CONNECTION = { connecting: 'Connecting', live: 'Live', offline: 'Reconnecting' } as const;
</script>

<svelte:head>
	<title>{problems > 0 ? `(${problems}) ` : ''}Watcher</title>
</svelte:head>

{#if unreachable}
	<div class="content"><p class="notice">API unreachable: {unreachable}</p></div>
{:else if bare}
	{#if auth.status}{@render children()}{/if}
{:else}
	<div class="shell" class:open={menuOpen}>
		<aside class="side">
			<a class="brand" href="/">Watcher</a>
			<nav aria-label="Main">
				{#each NAV as group (group.title)}
					<div class="group">{group.title}</div>
					{#each group.items as item (item.href)}
						<a href={item.href} class:active={active(item.href)} aria-current={active(item.href) ? 'page' : undefined}>
							{item.label}
							{#if item.href === '/problems' && problems > 0}<span class="badge">{problems}</span>{/if}
						</a>
					{/each}
				{/each}
				{#if admin}
					<div class="group">Configuration</div>
					{#each CONFIG_NAV as item (item.href)}
						<a href={item.href} class:active={active(item.href)} aria-current={active(item.href) ? 'page' : undefined}>
							{item.label}
						</a>
					{/each}
				{/if}
				{#if auth.status?.required}
					<div class="group">Settings</div>
					<a href="/account" class:active={active('/account')}>Account</a>
				{/if}
			</nav>
		</aside>
		<button class="scrim" type="button" aria-label="Close menu" onclick={() => (menuOpen = false)}></button>

		<div class="main">
			<header class="top">
				<button class="menu" type="button" aria-label="Menu" onclick={() => (menuOpen = !menuOpen)}>☰</button>
				<span class="conn" data-connection={live.connection} role="status">
					<span class="dot"></span>{CONNECTION[live.connection]}
				</span>
				{#if auth.status?.user}
					<span class="user">{auth.status.user.username}</span>
					<button class="btn" type="button" onclick={signOut}>Sign out</button>
				{/if}
			</header>
			{#if live.error && !live.loaded}
				<div class="content"><p class="notice">API unreachable: {live.error}</p></div>
			{:else}
				{@render children()}
			{/if}
		</div>
	</div>
{/if}

<style>
	.shell {
		display: grid;
		grid-template-columns: var(--sidebar) 1fr;
		min-height: 100vh;
	}

	.side {
		background: var(--side-bg);
		color: var(--side-text);
		padding: 0 0 16px;
		position: sticky;
		top: 0;
		height: 100vh;
		overflow-y: auto;
	}

	.brand {
		display: block;
		padding: 14px 16px;
		font-size: 16px;
		font-weight: 700;
		letter-spacing: 0.04em;
		color: #fff;
		text-decoration: none;
		border-bottom: 1px solid rgb(255 255 255 / 0.06);
	}

	.group {
		padding: 14px 16px 4px;
		font-size: 11px;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--side-muted);
	}

	nav a {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 6px 16px;
		color: var(--side-text);
		text-decoration: none;
		border-left: 3px solid transparent;
	}

	nav a:hover {
		background: var(--side-active);
		text-decoration: none;
	}

	nav a.active {
		background: var(--side-active);
		border-left-color: var(--link);
		color: #fff;
	}

	.badge {
		background: var(--crit);
		color: #fff;
		border-radius: 9px;
		padding: 0 7px;
		font-size: 11px;
		font-weight: 700;
		line-height: 17px;
	}

	.main {
		min-width: 0;
	}

	.top {
		display: flex;
		align-items: center;
		justify-content: flex-end;
		gap: 12px;
		height: 44px;
		padding: 0 20px;
		background: var(--panel);
		border-bottom: 1px solid var(--line);
	}

	.menu {
		display: none;
		margin-right: auto;
		font-size: 18px;
		background: none;
		border: 0;
		color: var(--text);
		cursor: pointer;
	}

	.conn {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		color: var(--muted);
		font-size: 12px;
	}

	.dot {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		background: var(--pending);
	}

	[data-connection='live'] .dot {
		background: var(--ok);
	}

	[data-connection='offline'] .dot {
		background: var(--warn);
	}

	.user {
		font-weight: 600;
	}

	.scrim {
		display: none;
	}

	@media (max-width: 720px) {
		.shell {
			grid-template-columns: 1fr;
		}
		.side {
			position: fixed;
			z-index: 20;
			left: 0;
			width: var(--sidebar);
			transform: translateX(-100%);
			transition: transform 150ms ease;
		}
		.open .side {
			transform: none;
		}
		.open .scrim {
			display: block;
			position: fixed;
			inset: 0;
			z-index: 10;
			background: rgb(0 0 0 / 0.35);
			border: 0;
		}
		.menu {
			display: block;
		}
		.top {
			padding: 0 10px;
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.side {
			transition: none;
		}
	}
</style>
