<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { untrack } from 'svelte';
	import { api, type Change, type Kind } from '#lib/api.ts';
	import ApplyDialog from '#lib/ApplyDialog.svelte';
	import { config, fileHref } from '#lib/config.svelte.ts';
	import ConfigHead from '#lib/ConfigHead.svelte';
	import ConfigTree from '#lib/ConfigTree.svelte';
	import { KINDS, SCHEMAS, SELECTOR_FIELDS, expandNames, fromForm, otherKeys, toForm, type Field, type FormValues, type ServiceRow } from '#lib/schema.ts';

	let kind = $derived(KINDS.find((k) => k.path === page.params.kind));
	let name = $derived(page.url.searchParams.get('name'));
	let object = $derived(kind && name !== null ? config.find(kind.kind, name) : undefined);
	/** Clone mode (`?clone=x`): a new form filled from x, saved into x's file. */
	let cloneName = $derived(name === null ? page.url.searchParams.get('clone') : null);
	let source = $derived(kind && cloneName !== null ? config.find(kind.kind, cloneName) : undefined);
	let raw = $derived.by<Record<string, unknown>>(() => {
		if (object) return object.fields;
		if (!source) return {};
		const { name: _, ...rest } = source.fields;
		return rest;
	});

	let values = $state<FormValues | null>(null);
	let pending = $state<Change[] | null>(null);
	let deleting = $state(false);
	let error = $state<string | null>(null);

	$effect(() => {
		config.load();
	});

	let scripts = $state<{ dirs: string[]; scripts: { name: string; dir: string }[] } | null>(null);
	$effect(() => {
		if (kind && SCHEMAS[kind.kind].some((g) => g.fields.some((f) => f.scripts)))
			api.configScripts().then((s) => (scripts = s), () => (scripts = null));
	});

	// Another object: start over.
	$effect(() => {
		void kind;
		void name;
		void cloneName;
		untrack(() => (values = null));
	});

	// Fill the form once the object is there; later reloads don't reset edits.
	$effect(() => {
		if (!kind || !config.loaded) return;
		if (cloneName !== null && !source) return;
		const fields = raw;
		const k = kind.kind;
		untrack(() => {
			if (values === null) values = toForm(k, fields);
		});
	});

	let others = $derived(kind ? otherKeys(kind.kind, raw) : []);

	/** Services of the host's roles (as written), by name: role. Host services with these names override them. */
	let roleServices = $derived.by(() => {
		const out = new Map<string, string>();
		if (kind?.kind !== 'host' || !values) return out;
		const listed = items(String(values.roles ?? ''));
		for (const role of config.of('role')) {
			const selected = role.fields.hosts !== undefined;
			if (!listed.includes(role.name) && !selected) continue;
			const svc = role.fields.service;
			if (svc && typeof svc === 'object') for (const s of Object.keys(svc)) out.set(s, role.name);
		}
		return out;
	});
	/** Problems on disk that point at this object, e.g. a command not found. */
	let own = $derived(
		object ? (config.overview?.diagnostics ?? []).filter((p) => p.path === object.path && p.line === object.line) : []
	);
	let missing = $derived(config.loaded && ((name !== null && !object) || (cloneName !== null && !source)));

	async function save(event: SubmitEvent) {
		event.preventDefault();
		if (!kind || !values) return;
		error = null;
		try {
			const fields = fromForm(kind.kind, raw, values);
			if (source) {
				pending = await clones(kind.kind, fields, source.path);
			} else {
				pending = [await api.editObject(kind.kind, name, fields)];
			}
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

	/** One clone per name, into the file of the original. Hosts cloned several times get their address from the name. */
	async function clones(k: Kind, fields: Record<string, unknown>, path: string): Promise<Change[]> {
		const list = expandNames(String(fields.name ?? ''));
		const edits =
			list.length === 0
				? [{ kind: k, name: null, fields, path }]
				: list.map((n) => {
						const f: Record<string, unknown> = { ...fields, name: n };
						if (k === 'host' && list.length > 1) delete f.address;
						return { kind: k, name: null, fields: f, path };
					});
		return (await api.editMany(edits)).changes;
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

	/** In clone mode, the name field takes several names. */
	function shown(f: Field): Field {
		if (f.key !== 'name' || cloneName === null) return f;
		return { ...f, label: 'Names', placeholder: 'web03, web[04-10]', help: 'one clone per name; hosts cloned several times: address = name' };
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
				<a href="/config/{kind.path}">{kind.label}</a> / {name ?? (cloneName !== null ? `Clone ${cloneName}` : `New ${kind.one}`)}
			</h1>
			{#if object}
				<span class="head-actions">
					{#if config.writable && object.name}<a class="btn" href="/config/{kind.path}/edit?clone={encodeURIComponent(object.name)}">Clone</a>{/if}
					{#if kind.kind !== 'sink' && object.name}<ConfigTree kind={kind.kind} name={object.name} />{/if}
					<a class="mono" href={fileHref(object.path, object.line)}>{object.path}:{object.line}</a>
				</span>
			{:else if source}
				<span class="muted mono">→ {source.path}</span>
			{:else if name === null}
				<span class="muted mono">→ {kind.file}</span>
			{/if}
		</div>
		<ConfigHead />
		{#each own as p, i (i)}
			<p class="notice problem" data-severity={p.severity}><span class="sev">{p.severity.toUpperCase()}</span> {p.message}</p>
		{/each}

		{#if missing}
			<p class="notice">No {kind.kind} "{name ?? cloneName}".</p>
		{:else if values}
			<form onsubmit={save}>
				<fieldset disabled={!config.writable}>
					{#each SCHEMAS[kind.kind] as group (group.title)}
						<section class="panel">
							<div class="panel-title">{group.title}</div>
							<div class="fields">
								{#each group.fields.map(shown) as f (f.key)}
									{@const id = `f-${f.key}`}
									<label for={id}>{f.label}{f.required ? ' *' : ''}</label>
									<div class="input">
										{#if f.type === 'bool'}
											<input {id} type="checkbox" bind:checked={values[f.key] as boolean} />
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
										{:else if f.type === 'selector'}
											{@const sel = values[f.key] as Record<string, string>}
											<div class="selector">
												{#if sel.list}
													<label class="sub" for="{id}-list">List</label>
													<input id="{id}-list" class="mono" bind:value={sel.list} />
												{:else}
													<label class="all"><input type="checkbox" checked={sel.all === 'true'}
														onchange={(e) => (sel.all = e.currentTarget.checked ? 'true' : '')} /> All {f.selects === 'service' ? 'services' : 'hosts'}</label>
													{#each SELECTOR_FIELDS[f.selects ?? 'host'] as [key, label, placeholder] (key)}
														<label class="sub" for="{id}-{key}">{label}</label>
														<input id="{id}-{key}" class:mono={key === 'expr'} {placeholder} bind:value={sel[key]} />
													{/each}
												{/if}
											</div>
										{:else if f.type === 'services'}
											<div class="rows">
												{#if rows<ServiceRow>(f.key).length > 0}
													<div class="row svc head"><span>Service</span><span>Check</span><span>For Each</span><span>Service Groups</span><span>Enabled</span><span></span></div>
												{/if}
												{#each rows<ServiceRow>(f.key) as row, i (i)}
													<div class="row svc">
														<input class="mono" placeholder="name" bind:value={row[0]} aria-label="Service" />
														<input class="mono" placeholder={row[0] || 'check'} bind:value={row[1]} aria-label="Check" list="checks-list" />
														<input class="mono" placeholder="host var" bind:value={row[2]} aria-label="For each" />
														<input placeholder="groups" bind:value={row[3]} aria-label="Service groups" />
														<select bind:value={row[4]} aria-label="Enabled">
															<option value="">—</option>
															<option value="yes">yes</option>
															<option value="no">no</option>
														</select>
														<button type="button" class="btn" aria-label="Remove"
															onclick={() => (values![f.key] = rows<ServiceRow>(f.key).filter((_, j) => j !== i))}>×</button>
														{#if kind.kind === 'host' && roleServices.has(row[0])}
															<span class="override">overrides role {roleServices.get(row[0])}</span>
														{/if}
													</div>
												{/each}
												<button type="button" class="btn small"
													onclick={() => (values![f.key] = [...rows<ServiceRow>(f.key), ['', '', '', '', '', '']])}>Add service</button>
												<datalist id="checks-list">{#each names('check') as c (c)}<option value={c}></option>{/each}</datalist>
											</div>
										{:else}
											<input
												{id}
												type="text"
												class:mono={f.type !== 'text'}
												inputmode={f.type === 'int' ? 'numeric' : undefined}
												placeholder={f.placeholder}
												required={f.required}
												list={f.scripts && scripts ? `${id}-scripts` : undefined}
												bind:value={values[f.key]}
											/>
											{#if f.scripts && scripts}
												<datalist id="{id}-scripts">
													{#each scripts.scripts as s (s.name)}<option value={s.name}>{s.dir}</option>{/each}
												</datalist>
											{/if}
										{/if}
										{#if f.help}<div class="help">{f.help}</div>{/if}
										{#if f.scripts && scripts}
											<div class="help">Check dirs: <span class="mono">{scripts.dirs.join(', ')}</span> ({scripts.scripts.length} scripts)</div>
										{/if}
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

	.notice.problem {
		margin: 0 0 12px;
		color: var(--text);
	}

	.sev {
		font-size: 11px;
		font-weight: 700;
		margin-right: 6px;
	}

	[data-severity='error'] .sev {
		color: var(--crit);
	}

	[data-severity='warning'] .sev {
		color: var(--unknown);
	}

	.head-actions {
		display: inline-flex;
		align-items: center;
		gap: 10px;
	}

	.selector {
		display: grid;
		grid-template-columns: 120px minmax(0, 1fr);
		gap: 4px 8px;
		align-items: center;
	}

	.selector .all {
		grid-column: 1 / -1;
		font-size: 12px;
	}

	.selector .sub {
		font-size: 12px;
		color: var(--muted);
	}

	.row.svc {
		grid-template-columns: 1.2fr 1fr 1fr 1.2fr 70px auto;
	}

	.row.svc > * {
		min-width: 0;
	}

	.row.svc.head {
		font-size: 11px;
		font-weight: 600;
		color: var(--muted);
	}

	.override {
		grid-column: 1 / -1;
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
