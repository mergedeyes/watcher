<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { untrack } from 'svelte';
	import { api, type Change, type Kind } from '#lib/api.ts';
	import ApplyDialog from '#lib/ApplyDialog.svelte';
	import { config, fileHref } from '#lib/config.svelte.ts';
	import ConfigHead from '#lib/ConfigHead.svelte';
	import { KINDS, SCHEMAS, fromForm, otherKeys, toForm, type Field, type FormValues } from '#lib/schema.ts';

	let kind = $derived(KINDS.find((k) => k.path === page.params.kind));
	let name = $derived(page.url.searchParams.get('name'));
	let object = $derived(kind && name !== null ? config.find(kind.kind, name) : undefined);
	let raw = $derived<Record<string, unknown>>(object?.fields ?? {});

	let values = $state<FormValues | null>(null);
	let pending = $state<Change[] | null>(null);
	let deleting = $state(false);
	let error = $state<string | null>(null);

	$effect(() => {
		config.load();
	});

	// Another object: start over.
	$effect(() => {
		void kind;
		void name;
		untrack(() => (values = null));
	});

	// Fill the form once the object is there; later reloads don't reset edits.
	$effect(() => {
		if (!kind || !config.loaded) return;
		const fields = raw;
		const k = kind.kind;
		untrack(() => {
			if (values === null) values = toForm(k, fields);
		});
	});

	let others = $derived(kind ? otherKeys(kind.kind, raw) : []);
	let missing = $derived(config.loaded && name !== null && !object);

	async function save(event: SubmitEvent) {
		event.preventDefault();
		if (!kind || !values) return;
		error = null;
		try {
			const fields = fromForm(kind.kind, raw, values);
			pending = [await api.editObject(kind.kind, name, fields)];
			deleting = false;
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	}

	async function remove() {
		if (!kind || name === null) return;
		error = null;
		try {
			pending = [await api.editObject(kind.kind, name, null)];
			deleting = true;
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	}

	function closed(applied: boolean) {
		pending = null;
		if (applied && kind) goto(`/config/${kind.path}`);
	}

	function names(k: Kind) {
		return config.of(k).map((o) => o.name);
	}

	function items(text: string) {
		return text
			.split(',')
			.map((s) => s.trim())
			.filter(Boolean);
	}

	function add(f: Field, item: string) {
		if (!values) return;
		const current = items(String(values[f.key] ?? ''));
		if (!current.includes(item)) values[f.key] = [...current, item].join(', ');
	}

	function rows<T>(key: string): T[] {
		return (values?.[key] as T[]) ?? [];
	}
</script>

<div class="content">
	{#if !kind}
		<p class="notice">Unknown object type.</p>
	{:else}
		<div class="page-head">
			<h1>
				<a href="/config/{kind.path}">{kind.label}</a> / {name ?? `New ${kind.one}`}
			</h1>
			{#if object}
				<a class="mono" href={fileHref(object.path, object.line)}>{object.path}:{object.line}</a>
			{:else if name === null}
				<span class="muted mono">→ {kind.kind === 'template' ? 'ui/templates.toml' : `ui/${kind.path}.toml`}</span>
			{/if}
		</div>
		<ConfigHead />

		{#if missing}
			<p class="notice">No {kind.kind} "{name}".</p>
		{:else if values}
			<form onsubmit={save}>
				<fieldset disabled={!config.writable}>
					{#each SCHEMAS[kind.kind] as group (group.title)}
						<section class="panel">
							<div class="panel-title">{group.title}</div>
							<div class="fields">
								{#each group.fields as f (f.key)}
									{@const id = `f-${f.key}`}
									<label for={id}>{f.label}{f.required ? ' *' : ''}</label>
									<div class="input">
										{#if f.type === 'bool'}
											<input {id} type="checkbox" bind:checked={values[f.key] as boolean} />
										{:else if f.type === 'tristate'}
											<select {id} bind:value={values[f.key]}>
												<option value={null}>—</option>
												<option value={true}>yes</option>
												<option value={false}>no</option>
											</select>
										{:else if f.type === 'select'}
											<select {id} bind:value={values[f.key]}>
												<option value="">— (auto)</option>
												{#each f.options ?? [] as o (o)}<option value={o}>{o}</option>{/each}
											</select>
										{:else if f.type === 'vars'}
											<div class="rows">
												{#each rows<[string, string]>(f.key) as row, i (i)}
													<div class="row two">
														<input class="mono" placeholder="name" bind:value={row[0]} aria-label="Variable name" />
														<input class="mono" placeholder="value" bind:value={row[1]} aria-label="Variable value" />
														<button type="button" class="btn" aria-label="Remove"
															onclick={() => (values![f.key] = rows<[string, string]>(f.key).filter((_, j) => j !== i))}>×</button>
													</div>
												{/each}
												<button type="button" class="btn small"
													onclick={() => (values![f.key] = [...rows<[string, string]>(f.key), ['', '']])}>Add variable</button>
											</div>
										{:else if f.type === 'thresholds'}
											<div class="rows">
												{#each rows<[string, string, string]>(f.key) as row, i (i)}
													<div class="row three">
														<input class="mono" placeholder="metric" bind:value={row[0]} aria-label="Metric" />
														<input class="mono" placeholder="warn, e.g. > 100" bind:value={row[1]} aria-label="Warning" />
														<input class="mono" placeholder="crit, e.g. > 500" bind:value={row[2]} aria-label="Critical" />
														<button type="button" class="btn" aria-label="Remove"
															onclick={() => (values![f.key] = rows<[string, string, string]>(f.key).filter((_, j) => j !== i))}>×</button>
													</div>
												{/each}
												<button type="button" class="btn small"
													onclick={() => (values![f.key] = [...rows<[string, string, string]>(f.key), ['', '', '']])}>Add threshold</button>
											</div>
										{:else}
											<input
												{id}
												type="text"
												class:mono={f.type !== 'text'}
												inputmode={f.type === 'int' ? 'numeric' : undefined}
												placeholder={f.placeholder}
												required={f.required}
												bind:value={values[f.key]}
											/>
										{/if}
										{#if f.help}<div class="help">{f.help}</div>{/if}
										{#if f.suggestOne}
											{@const current = String(values[f.key] ?? '').trim()}
											{@const free = names(f.suggestOne).filter((n) => n !== current)}
											{#if free.length > 0}
												<div class="suggest">
													{#each free.slice(0, 20) as n (n)}
														<button type="button" class="tag" onclick={() => (values![f.key] = n)}>{n}</button>
													{/each}
												</div>
											{/if}
										{/if}
										{#if f.suggest}
											{@const chosen = items(String(values[f.key] ?? ''))}
											{@const free = names(f.suggest).filter((n) => !chosen.includes(n))}
											{#if free.length > 0}
												<div class="suggest">
													{#each free.slice(0, 20) as n (n)}
														<button type="button" class="tag" onclick={() => add(f, n)}>+ {n}</button>
													{/each}
												</div>
											{/if}
										{/if}
									</div>
								{/each}
							</div>
						</section>
					{/each}
				</fieldset>

				{#if others.length > 0}
					<p class="notice">
						Kept as written: <span class="mono">{others.join(', ')}</span>
						{#if object}— <a href={fileHref(object.path, object.line)}>edit in file</a>{/if}
					</p>
				{/if}
				{#if error}<p class="notice error" role="alert">{error}</p>{/if}

				{#if config.writable}
					<div class="buttons">
						{#if object}
							<button type="button" class="btn danger" onclick={remove}>Delete</button>
						{/if}
						<span class="spacer"></span>
						<a class="btn" href="/config/{kind.path}">Cancel</a>
						<button type="submit" class="btn primary">Save</button>
					</div>
				{/if}
			</form>
		{/if}
	{/if}
</div>

<ApplyDialog
	changes={pending}
	title={deleting ? `Delete ${kind?.one ?? ''} ${name ?? ''}` : `Save ${kind?.one ?? ''} ${name ?? ''}`}
	onclose={closed}
	onconflict={(c) => {
		error = `${c.path} changed on disk; reloaded. Save again.`;
		config.load();
	}}
/>

<style>
	fieldset {
		border: 0;
		margin: 0;
		padding: 0;
		min-width: 0;
	}

	section + section {
		margin-top: 12px;
	}

	.fields {
		display: grid;
		grid-template-columns: 160px minmax(0, 1fr);
		gap: 8px 14px;
		padding: 12px;
		align-items: start;
	}

	label {
		padding-top: 5px;
		font-weight: 500;
		color: var(--muted);
	}

	.input > input[type='text'],
	.input > select {
		width: 100%;
		max-width: 640px;
	}

	.input > input[type='checkbox'] {
		margin-top: 7px;
	}

	.help {
		margin-top: 2px;
		font-size: 11px;
		color: var(--muted);
	}

	.suggest {
		display: flex;
		flex-wrap: wrap;
		gap: 4px;
		margin-top: 4px;
	}

	.suggest .tag {
		cursor: pointer;
		font: inherit;
		font-size: 11px;
	}

	.suggest .tag:hover {
		color: var(--text);
		border-color: var(--muted);
	}

	.rows {
		display: grid;
		gap: 4px;
		max-width: 640px;
	}

	.row {
		display: grid;
		gap: 4px;
	}

	.row.two {
		grid-template-columns: minmax(0, 1fr) minmax(0, 2fr) auto;
	}

	.row.three {
		grid-template-columns: minmax(0, 1fr) minmax(0, 1fr) minmax(0, 1fr) auto;
	}

	.btn.small {
		justify-self: start;
		font-size: 12px;
		padding: 3px 8px;
	}

	.notice {
		margin: 12px 0 0;
	}

	.error {
		border-color: var(--crit);
		color: var(--crit);
	}

	.buttons {
		display: flex;
		gap: 8px;
		margin-top: 14px;
	}

	.spacer {
		flex: 1;
	}

	.btn.danger {
		color: var(--crit);
		border-color: var(--crit);
	}

	@media (max-width: 720px) {
		.fields {
			grid-template-columns: 1fr;
			gap: 2px;
		}
		label {
			padding-top: 8px;
		}
	}
</style>
