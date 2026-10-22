<script lang="ts">
	import { beforeNavigate, goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api, ApiError, type Change, type Changes, type ConfigFile, type Conflict, type Problem } from '#lib/api.ts';
	import ApplyDialog from '#lib/ApplyDialog.svelte';
	import { config, fileHref } from '#lib/config.svelte.ts';
	import TomlEditor from '#lib/TomlEditor.svelte';

	let path = $derived(page.url.searchParams.get('path') ?? '');
	let line = $derived(Number(page.url.searchParams.get('line') ?? 0));

	/** The file as last read; null for a new file. */
	let file = $state<ConfigFile | null>(null);
	let loaded = $state(false);
	let text = $state('');
	/** Hash the edit is based on. */
	let base = $state<string | null>(null);
	let problems = $state<Problem[]>([]);
	let changes = $state<Changes | null>(null);
	let conflict = $state<Conflict | null>(null);
	let pending = $state<Change[] | null>(null);
	let deleting = $state(false);
	let error = $state<string | null>(null);
	let saved = $state(false);
	let editor = $state<TomlEditor>();

	let dirty = $derived(loaded && text !== (file?.text ?? ''));
	let mine = $derived(problems.filter((p) => p.path === path));
	let elsewhere = $derived(problems.filter((p) => p.path !== path));

	async function load() {
		error = null;
		conflict = null;
		try {
			file = await api.configFile(path);
		} catch (e) {
			if (!(e instanceof ApiError && e.status === 404)) {
				error = e instanceof Error ? e.message : String(e);
				return;
			}
			file = null;
		}
		text = file?.text ?? '';
		base = file?.hash ?? null;
		loaded = true;
	}

	$effect(() => {
		void path;
		loaded = false;
		config.load();
		load();
	});

	// Check while typing.
	$effect(() => {
		if (!loaded) return;
		const change: Change = { path, base, text };
		const timer = setTimeout(async () => {
			try {
				const p = await api.preview([change]);
				problems = p.diagnostics;
				changes = p.valid ? p.changes : null;
			} catch (e) {
				if (e instanceof ApiError && e.status === 409) conflict = e.body as unknown as Conflict;
				else error = e instanceof Error ? e.message : String(e);
			}
		}, 500);
		return () => clearTimeout(timer);
	});

	beforeNavigate((nav) => {
		if (dirty && !saved && nav.type !== 'leave' && !confirm('Discard unsaved changes?')) nav.cancel();
	});

	function save() {
		saved = false;
		deleting = false;
		pending = [{ path, base, text }];
	}

	function remove() {
		deleting = true;
		pending = [{ path, base, text: null }];
	}

	async function closed(applied: boolean) {
		pending = null;
		if (!applied) return;
		if (deleting) {
			saved = true;
			goto('/config/files');
			return;
		}
		await load();
		saved = true;
	}

	/** Take the version on disk, dropping this edit. */
	function theirs() {
		if (!conflict) return;
		text = conflict.current?.text ?? '';
		file = conflict.current;
		base = conflict.current?.hash ?? null;
		conflict = null;
	}

	/** Keep this edit; saving then replaces the version on disk. */
	function keepMine() {
		if (!conflict) return;
		file = conflict.current;
		base = conflict.current?.hash ?? null;
		conflict = null;
	}

	const DELTAS = ['hosts', 'services', 'contacts', 'sinks'] as const;

	function summary(c: Changes) {
		return DELTAS.flatMap((k) => {
			const d = c[k];
			const parts = [
				d.added.length ? `+${d.added.length}` : '',
				d.changed.length ? `~${d.changed.length}` : '',
				d.removed.length ? `−${d.removed.length}` : ''
			].filter(Boolean);
			return parts.length ? [`${k} ${parts.join(' ')}`] : [];
		}).join(' · ');
	}
</script>

<div class="content">
	<div class="page-head">
		<h1><a href="/config/files">Files</a> / <span class="mono">{path}</span>{dirty ? ' *' : ''}</h1>
		{#if config.writable}
			<div class="buttons">
				{#if file}<button type="button" class="btn danger" onclick={remove}>Delete File</button>{/if}
				<button type="button" class="btn" onclick={load} disabled={!dirty}>Revert</button>
				<button type="button" class="btn primary" onclick={save} disabled={!dirty && file !== null}>Save</button>
			</div>
		{/if}
	</div>

	{#if error}<p class="notice error" role="alert">{error}</p>{/if}
	{#if config.overview && !config.overview.writable}
		<p class="notice">Read-only: <code>config_mode = "{config.overview.mode}"</code></p>
	{/if}
	{#if conflict}
		<div class="notice conflict" role="alert">
			<strong>{conflict.current ? 'Changed on disk.' : 'Deleted on disk.'}</strong>
			Differences to the disk version are marked; revert chunks, or:
			<button type="button" class="btn" onclick={theirs}>Use Disk Version</button>
			<button type="button" class="btn" onclick={keepMine}>Keep Mine</button>
		</div>
	{/if}
	{#if loaded && !file}<p class="notice">New file.</p>{/if}

	{#if loaded}
		<div class="editor">
			<TomlEditor
				bind:this={editor}
				bind:value={text}
				readonly={!config.writable}
				problems={mine}
				original={conflict ? (conflict.current?.text ?? '') : null}
				{line}
			/>
		</div>

		<div class="status">
			{#if mine.length === 0 && elsewhere.length === 0}
				<span class="ok">Config OK</span>
			{/if}
			{#if changes && dirty}
				<span class="muted">{summary(changes) || 'no effective changes'}</span>
			{/if}
			{#if saved && !dirty}<span class="ok">Saved</span>{/if}
		</div>
		{#if problems.length > 0}
			<ul class="problems">
				{#each mine as p, i (i)}
					<li data-severity={p.severity}>
						<button type="button" class="line mono" onclick={() => editor?.goto(p.line)}>{p.line}</button>
						<span class="sev">{p.severity.toUpperCase()}</span>
						{p.message}
					</li>
				{/each}
				{#each elsewhere as p, i (i)}
					<li data-severity={p.severity}>
						<a class="line mono" href={p.path ? fileHref(p.path, p.line) : undefined}>{p.path}:{p.line}</a>
						<span class="sev">{p.severity.toUpperCase()}</span>
						{p.message}
					</li>
				{/each}
			</ul>
		{/if}
	{/if}
</div>

<ApplyDialog
	changes={pending}
	title={deleting ? `Delete ${path}` : `Save ${path}`}
	onclose={closed}
	onconflict={(c) => (conflict = c)}
/>

<style>
	.buttons {
		display: flex;
		gap: 8px;
	}

	/* The editor scrolls inside; the page keeps its header. */
	.editor {
		height: max(360px, calc(100vh - 230px));
	}

	.status {
		display: flex;
		gap: 14px;
		margin-top: 8px;
		font-size: 12px;
	}

	.ok {
		color: var(--ok);
		font-weight: 600;
	}

	.notice {
		margin: 0 0 10px;
	}

	.error,
	.conflict {
		border-color: var(--crit);
	}

	.error,
	.conflict strong {
		color: var(--crit);
	}

	.conflict .btn {
		margin-left: 6px;
	}

	.btn.danger {
		color: var(--crit);
		border-color: var(--crit);
	}

	.problems {
		list-style: none;
		margin: 8px 0 0;
		padding: 0;
		display: grid;
		gap: 3px;
	}

	.line {
		display: inline-block;
		min-width: 40px;
		font: inherit;
		font-family: var(--mono);
		font-size: 12px;
		color: var(--link);
		background: none;
		border: 0;
		padding: 0;
		cursor: pointer;
		text-align: left;
	}

	.sev {
		display: inline-block;
		min-width: 64px;
		font-size: 11px;
		font-weight: 700;
	}

	[data-severity='error'] .sev {
		color: var(--crit);
	}

	[data-severity='warning'] .sev {
		color: var(--unknown);
	}
</style>
