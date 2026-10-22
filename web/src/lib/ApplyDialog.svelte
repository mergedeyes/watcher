<!--
	Preview and apply config changes: errors, or the effective changes
	(hosts, services, contacts, sinks) with a confirm.
-->
<script lang="ts">
	import { api, ApiError, type Change, type Changes, type Conflict, type Preview, type Problem } from './api.ts';

	interface Props {
		/** Opens the dialog when set. */
		changes: Change[] | null;
		title?: string;
		/** Closed; `applied` after a successful apply. */
		onclose: (applied: boolean) => void;
		/** A file changed since it was read. */
		onconflict?: (conflict: Conflict) => void;
	}

	let { changes, title = 'Apply Changes', onclose, onconflict }: Props = $props();

	let dialog: HTMLDialogElement;
	let preview = $state<Preview | null>(null);
	let error = $state<string | null>(null);
	let busy = $state(false);
	/** Reported by the close event. */
	let applied = false;

	$effect(() => {
		if (!changes) return;
		const current = changes;
		preview = null;
		error = null;
		dialog.showModal();
		api.preview(current).then(
			(p) => {
				if (changes === current) preview = p;
			},
			(e) => failed(e)
		);
	});

	function failed(e: unknown) {
		if (e instanceof ApiError && e.status === 409 && onconflict) {
			dialog.close();
			onconflict(e.body as unknown as Conflict);
			return;
		}
		error = e instanceof Error ? e.message : String(e);
	}

	async function apply() {
		if (!changes) return;
		busy = true;
		try {
			await api.apply(changes);
			applied = true;
			dialog.close();
		} catch (e) {
			failed(e);
		} finally {
			busy = false;
		}
	}

	function cancel() {
		dialog.close();
	}

	const SECTIONS: { key: keyof Changes; label: string }[] = [
		{ key: 'hosts', label: 'Hosts' },
		{ key: 'services', label: 'Services' },
		{ key: 'contacts', label: 'Contacts' },
		{ key: 'sinks', label: 'Sinks' }
	];

	function empty(c: Changes) {
		return SECTIONS.every(({ key }) => c[key].added.length + c[key].removed.length + c[key].changed.length === 0);
	}

	function where(p: Problem) {
		return p.path ? `${p.path}${p.line ? `:${p.line}` : ''}` : '';
	}
</script>

<dialog
	bind:this={dialog}
	onclose={() => {
		onclose(applied);
		applied = false;
	}}
	aria-labelledby="apply-title"
>
	<div class="head" id="apply-title">{title}</div>
	<div class="body">
		{#if error}
			<p class="error" role="alert">{error}</p>
		{:else if !preview}
			<p class="muted">Checking…</p>
		{:else}
			{#if preview.valid}
				{#if empty(preview.changes)}
					<p class="muted">No effective changes.</p>
				{:else}
					<table class="grid">
						<tbody>
							{#each SECTIONS as { key, label } (key)}
								{@const d = preview.changes[key]}
								{#each d.added as name (name)}
									<tr><td class="op add">+</td><td>{label}</td><td class="mono">{name}</td></tr>
								{/each}
								{#each d.changed as name (name)}
									<tr><td class="op chg">~</td><td>{label}</td><td class="mono">{name}</td></tr>
								{/each}
								{#each d.removed as name (name)}
									<tr><td class="op del">−</td><td>{label}</td><td class="mono">{name}</td></tr>
								{/each}
							{/each}
						</tbody>
					</table>
				{/if}
			{:else}
				<p class="error">Config has errors; nothing applied.</p>
			{/if}
			{#if preview.diagnostics.length > 0}
				<ul class="problems">
					{#each preview.diagnostics as p, i (i)}
						<li data-severity={p.severity}>
							<span class="sev">{p.severity === 'error' ? 'ERROR' : 'WARNING'}</span>
							<span class="mono">{where(p)}</span>
							{p.message}
						</li>
					{/each}
				</ul>
			{/if}
		{/if}
	</div>
	<div class="foot">
		<button type="button" class="btn" onclick={cancel}>Cancel</button>
		<button type="button" class="btn primary" onclick={apply} disabled={!preview?.valid || busy}>
			{busy ? 'Applying…' : 'Apply'}
		</button>
	</div>
</dialog>

<style>
	dialog {
		width: min(640px, calc(100vw - 32px));
		padding: 0;
		border: 1px solid var(--line);
		border-radius: var(--radius);
		background: var(--panel);
		color: var(--text);
	}

	dialog::backdrop {
		background: rgb(0 0 0 / 0.35);
	}

	.head {
		padding: 10px 14px;
		font-weight: 600;
		border-bottom: 1px solid var(--line);
	}

	.body {
		padding: 12px 14px;
		max-height: 60vh;
		overflow: auto;
	}

	.body > p {
		margin: 0;
	}

	.foot {
		display: flex;
		justify-content: flex-end;
		gap: 8px;
		padding: 10px 14px;
		border-top: 1px solid var(--line);
	}

	.body table {
		width: auto;
		min-width: 60%;
	}

	.op {
		width: 18px;
		font-weight: 700;
		text-align: center;
	}
	.add {
		color: var(--ok);
	}
	.chg {
		color: var(--warn);
	}
	.del {
		color: var(--crit);
	}

	.problems {
		list-style: none;
		margin: 10px 0 0;
		padding: 0;
		display: grid;
		gap: 4px;
	}

	.sev {
		display: inline-block;
		min-width: 64px;
		font-size: 11px;
		font-weight: 700;
	}

	[data-severity='error'] .sev,
	.error {
		color: var(--crit);
	}

	[data-severity='warning'] .sev {
		color: var(--unknown);
	}
</style>
