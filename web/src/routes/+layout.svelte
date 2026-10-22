<script lang="ts">
	import '../app.css';
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import { live } from '#lib/live.svelte.ts';

	let { children } = $props();

	onMount(() => live.start());

	const nav = [
		{ href: '/', label: 'Problems' },
		{ href: '/hosts', label: 'Hosts' },
		{ href: '/services', label: 'Services' }
	];

	function active(href: string) {
		const path = page.url.pathname;
		return href === '/' ? path === '/' : path.startsWith(href);
	}

	let problems = $derived(
		live.services.filter((s) => s.state !== null && s.state !== 'ok').length
	);

	const CONNECTION = {
		connecting: 'Connecting',
		live: 'Live',
		offline: 'Reconnecting'
	} as const;
</script>

<svelte:head>
	<title>{problems > 0 ? `(${problems}) ` : ''}Watcher</title>
</svelte:head>

<header class="rail">
	<div class="rail-inner">
		<a class="wordmark legend" href="/">Watcher</a>
		<nav aria-label="Main">
			{#each nav as item (item.href)}
				<a href={item.href} class="legend" aria-current={active(item.href) ? 'page' : undefined}>
					{item.label}
					{#if item.href === '/' && problems > 0}
						<span class="count data">{problems}</span>
					{/if}
				</a>
			{/each}
		</nav>
		<span class="connection" data-connection={live.connection} role="status">
			<span class="pilot" aria-hidden="true"></span>
			<span class="legend">{CONNECTION[live.connection]}</span>
		</span>
	</div>
</header>

{#if live.error && !live.loaded}
	<div class="page">
		<p class="notice">
			Can't reach the Watcher API: {live.error}. Check that <code>watcher run</code> is running; this
			page retries on its own.
		</p>
	</div>
{:else}
	{@render children()}
{/if}

<style>
	.rail {
		background: var(--ink);
		color: var(--enclosure);
		border-bottom: 3px solid var(--rule);
	}

	@media (prefers-color-scheme: dark) {
		.rail {
			background: #0b0c0c;
			color: var(--ink);
		}
	}

	.rail-inner {
		max-width: var(--max);
		margin: 0 auto;
		padding: 0 var(--gutter);
		display: flex;
		align-items: center;
		gap: 28px;
		min-height: 52px;
	}

	.wordmark {
		font-size: 18px;
		letter-spacing: 0.22em;
		text-decoration: none;
	}

	nav {
		display: flex;
		gap: 4px;
		flex: 1;
		min-width: 0;
		overflow-x: auto;
	}

	nav a {
		display: inline-flex;
		align-items: center;
		gap: 8px;
		padding: 15px 10px 13px;
		font-size: 13px;
		text-decoration: none;
		opacity: 0.7;
		border-bottom: 2px solid transparent;
		white-space: nowrap;
	}

	nav a:hover {
		opacity: 1;
	}

	nav a[aria-current='page'] {
		opacity: 1;
		border-bottom-color: currentColor;
	}

	.count {
		background: var(--crit);
		color: var(--on-crit);
		border-radius: 2px;
		padding: 0 5px;
		font-size: 12px;
		line-height: 18px;
	}

	.connection {
		display: inline-flex;
		align-items: center;
		gap: 8px;
		font-size: 12px;
		opacity: 0.85;
		white-space: nowrap;
	}

	/* Pilot light: steady green while live. */
	.pilot {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		background: var(--pending);
	}

	[data-connection='live'] .pilot {
		background: #3fbf6f;
		box-shadow: 0 0 8px #3fbf6f;
	}

	[data-connection='offline'] .pilot {
		background: var(--warn);
		box-shadow: 0 0 8px var(--warn);
	}

	@media (max-width: 640px) {
		.rail-inner {
			gap: 12px;
		}
		.wordmark {
			font-size: 15px;
			letter-spacing: 0.14em;
		}
		.connection .legend {
			display: none;
		}
		nav a {
			padding: 15px 6px 13px;
			font-size: 12px;
		}
	}
</style>
