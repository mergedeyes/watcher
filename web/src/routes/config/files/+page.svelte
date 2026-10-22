<script lang="ts">
	import { goto } from '$app/navigation';
	import { config, fileHref } from '#lib/config.svelte.ts';
	import ConfigHead from '#lib/ConfigHead.svelte';
	import { live } from '#lib/live.svelte.ts';

	let path = $state('');

	$effect(() => {
		void live.reloads;
		config.load();
	});

	let counts = $derived.by(() => {
		const c = new Map<string, number>();
		for (const o of config.objects) c.set(o.path, (c.get(o.path) ?? 0) + 1);
		return c;
	});

	function size(n: number) {
		return n < 1024 ? `${n} B` : `${(n / 1024).toFixed(1)} KiB`;
	}

	function create(event: SubmitEvent) {
		event.preventDefault();
		let p = path.trim().replace(/^\/+/, '');
		if (!p) return;
		if (!p.endsWith('.toml')) p += '.toml';
		goto(fileHref(p));
	}
</script>

<div class="content">
	<div class="page-head">
		<h1>Files <span class="muted">{config.overview?.files.length ?? ''}</span></h1>
		{#if config.writable}
			<form class="new" onsubmit={create}>
				<input class="mono" placeholder="path, e.g. hosts/db.toml" bind:value={path} aria-label="New file path" />
				<button type="submit" class="btn primary">New File</button>
			</form>
		{/if}
	</div>
	<ConfigHead />

	{#if config.overview}
		<section class="panel">
			<div class="panel-title">
				<span class="mono">{config.overview.dir}</span>
				<span class="muted">mode: {config.overview.mode}</span>
			</div>
			<table class="grid">
				<thead>
					<tr>
						<th>File</th>
						<th>Objects</th>
						<th class="hide-narrow">Size</th>
					</tr>
				</thead>
				<tbody>
					{#each config.overview.files as f (f.path)}
						<tr>
							<td><a class="mono" href={fileHref(f.path)}>{f.path}</a></td>
							<td>{counts.get(f.path) ?? 0}</td>
							<td class="hide-narrow nowrap">{size(f.size)}</td>
						</tr>
					{:else}
						<tr><td colspan="3" class="empty">No files.</td></tr>
					{/each}
				</tbody>
			</table>
		</section>
	{/if}
</div>

<style>
	.new {
		display: flex;
		gap: 8px;
	}

	.new input {
		width: 260px;
		max-width: 100%;
	}
</style>
