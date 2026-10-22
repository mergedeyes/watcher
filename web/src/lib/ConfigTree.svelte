<!--
	"Config Tree" button and side panel: where an object and its values come
	from, and where it is used. Objects link to their form, places to the
	file editor at the line.
-->
<script lang="ts">
	import { api, ApiError, serviceHref, type TreeKind, type TreeNode } from './api.ts';
	import { editHref, fileHref } from './config.svelte.ts';
	import { KINDS } from './schema.ts';

	let { kind, name }: { kind: TreeKind; name: string } = $props();

	let open = $state(false);
	let tree = $state<TreeNode | null>(null);
	let error = $state<string | null>(null);

	async function show() {
		open = true;
		tree = null;
		error = null;
		try {
			tree = await api.configTree(kind, name);
		} catch (e) {
			error = e instanceof ApiError && e.status === 404 ? 'Not in the running config.' : e instanceof Error ? e.message : String(e);
		}
	}

	/** Where an object node links to. */
	function href(node: TreeNode): string | undefined {
		if (!node.name) return undefined;
		if (node.kind === 'service') {
			const [host, ...rest] = node.name.split('/');
			return serviceHref(host ?? '', rest.join('/'));
		}
		// Unnamed notify rules have no form.
		if (node.kind === 'notify' && node.name.startsWith('rule at ')) return undefined;
		const path = KINDS.find((k) => k.kind === node.kind)?.path;
		return path ? editHref(path, node.name) : undefined;
	}

	function onkeydown(e: KeyboardEvent) {
		if (open && e.key === 'Escape') open = false;
	}
</script>

<svelte:window {onkeydown} />

<button type="button" class="btn" onclick={show}>Config Tree</button>

{#snippet item(node: TreeNode, depth: number)}
	{@const link = href(node)}
	{@const children = node.children ?? []}
	<li class:section={node.kind === 'section'}>
		{#if children.length > 0 && node.kind !== 'value'}
			<details open={depth < 2}>
				<summary>{@render line(node, link, depth)}</summary>
				<ul>{#each children as c, i (i)}{@render item(c, depth + 1)}{/each}</ul>
			</details>
		{:else}
			<div class="leaf">
				{@render line(node, link, depth)}
				{#if node.kind === 'value' && children.length > 0}
					{@const src = children[0]!}
					<span class="from">← {@render label(src, href(src))}</span>
				{/if}
			</div>
		{/if}
	</li>
{/snippet}

{#snippet label(node: TreeNode, link: string | undefined)}
	{#if link}<a href={link} onclick={() => (open = false)}>{node.label}</a>{:else}{node.label}{/if}
	{#if node.file && node.kind !== 'value'}
		<a class="place mono" href={fileHref(node.file, node.line ?? 0)} onclick={() => (open = false)}>{node.file}{node.line ? `:${node.line}` : ''}</a>
	{/if}
{/snippet}

{#snippet line(node: TreeNode, link: string | undefined, depth: number)}
	<!-- Labels below the root name their kind already. -->
	{#if depth === 0}<span class="kind">{node.kind}</span>{/if}
	{#if node.kind === 'value'}
		<span class="mono key">{node.label}</span>
		{#if node.detail}<span class="mono val">{node.detail}</span>{/if}
	{:else}
		<span class="label">{@render label(node, link)}</span>
		{#if node.detail}<span class="detail">{node.detail}</span>{/if}
	{/if}
{/snippet}

{#if open}
	<button type="button" class="scrim" aria-label="Close" onclick={() => (open = false)}></button>
	<aside class="drawer" aria-label="Config Tree">
		<header>
			<span class="title">Config Tree <span class="muted mono">{kind} {name}</span></span>
			<button type="button" class="btn" onclick={() => (open = false)}>Close</button>
		</header>
		<div class="body">
			{#if error}
				<p class="notice">{error}</p>
			{:else if !tree}
				<p class="notice">Loading…</p>
			{:else}
				<ul class="tree">{@render item(tree, 0)}</ul>
			{/if}
		</div>
	</aside>
{/if}

<style>
	.scrim {
		position: fixed;
		inset: 0;
		background: rgb(0 0 0 / 0.25);
		border: 0;
		z-index: 40;
	}

	.drawer {
		position: fixed;
		top: 0;
		right: 0;
		bottom: 0;
		width: min(640px, 100vw);
		background: var(--panel);
		border-left: 1px solid var(--line);
		box-shadow: -4px 0 16px rgb(0 0 0 / 0.15);
		z-index: 41;
		display: flex;
		flex-direction: column;
	}

	header {
		display: flex;
		justify-content: space-between;
		align-items: center;
		gap: 8px;
		padding: 10px 14px;
		border-bottom: 1px solid var(--line);
	}

	.title {
		font-weight: 600;
	}

	.title .muted {
		font-weight: 400;
		font-size: 12px;
		margin-left: 6px;
	}

	.body {
		overflow: auto;
		padding: 8px 14px 24px;
		font-size: 13px;
	}

	ul {
		list-style: none;
		margin: 0;
		padding: 0 0 0 16px;
		border-left: 1px solid var(--line);
	}

	ul.tree {
		padding: 0;
		border: 0;
	}

	li {
		margin: 2px 0;
	}

	summary {
		cursor: pointer;
	}

	.leaf {
		padding-left: 14px;
	}

	.section > details > summary .label {
		font-weight: 600;
		text-transform: capitalize;
	}

	.kind {
		display: inline-block;
		min-width: 0;
		margin-right: 4px;
		font-size: 10px;
		font-weight: 700;
		text-transform: uppercase;
		color: var(--muted);
	}

	.detail,
	.from {
		margin-left: 6px;
		color: var(--muted);
	}

	.key {
		color: var(--muted);
	}

	.val {
		margin-left: 6px;
		word-break: break-all;
	}

	.place {
		margin-left: 6px;
		font-size: 11px;
		color: var(--muted);
	}

	.place:hover {
		color: var(--link);
	}
</style>
