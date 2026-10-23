<!-- "Edit Config" on status pages: one link, or a menu when an object is defined in several places. -->
<script module lang="ts">
	export interface EditTarget {
		label: string;
		href: string;
		/** What editing there changes, e.g. "all hosts in the group". */
		hint?: string;
	}
</script>

<script lang="ts">
	let { targets }: { targets: EditTarget[] } = $props();

	let open = $state(false);
	let root = $state<HTMLElement>();

	function onclick(e: MouseEvent) {
		if (open && !root?.contains(e.target as Node)) open = false;
	}

	function onkeydown(e: KeyboardEvent) {
		if (open && e.key === 'Escape') open = false;
	}
</script>

<svelte:window {onclick} {onkeydown} />

{#if targets.length === 1}
	<a class="btn" href={targets[0]!.href}>Edit Config</a>
{:else if targets.length > 1}
	<span class="menu" bind:this={root}>
		<button type="button" class="btn" aria-haspopup="menu" aria-expanded={open} onclick={() => (open = !open)}>Edit Config ▾</button>
		{#if open}
			<ul role="menu">
				{#each targets as t (t.href)}
					<li role="none">
						<a role="menuitem" href={t.href} onclick={() => (open = false)}>
							<span>{t.label}</span>
							{#if t.hint}<span class="hint">{t.hint}</span>{/if}
						</a>
					</li>
				{/each}
			</ul>
		{/if}
	</span>
{/if}

<style>
	.menu {
		position: relative;
		display: inline-block;
	}

	ul {
		position: absolute;
		right: 0;
		top: calc(100% + 2px);
		z-index: 30;
		min-width: 260px;
		margin: 0;
		padding: 2px 0;
		list-style: none;
		background: var(--panel);
		border: 1px solid var(--line);
		border-radius: 3px;
		box-shadow: 0 4px 12px rgb(0 0 0 / 0.15);
	}

	a[role='menuitem'] {
		display: flex;
		justify-content: space-between;
		gap: 12px;
		padding: 6px 10px;
		color: var(--text);
		text-decoration: none;
		font-size: 13px;
	}

	a[role='menuitem']:hover {
		background: color-mix(in srgb, var(--link) 14%, transparent);
	}

	.hint {
		color: var(--muted);
		font-size: 11px;
		white-space: nowrap;
	}
</style>
