<script lang="ts">
	import { api, hostHref, serviceHref, type ActionBody, type Downtime, type Flag } from '#lib/api.ts';
	import { actions } from '#lib/actions.svelte.ts';
	import Combobox from '#lib/Combobox.svelte';
	import { datetime, duration } from '#lib/format.ts';
	import { live } from '#lib/live.svelte.ts';

	let downtimes = $state<Downtime[]>([]);
	let flags = $state<Flag[]>([]);
	let groups = $state<Awaited<ReturnType<typeof api.groups>> | null>(null);
	let error = $state<string | null>(null);

	$effect(() => {
		void live.reloads;
		load();
	});

	async function load() {
		try {
			[downtimes, flags, groups] = await Promise.all([api.downtimes(), api.flags(), api.groups()]);
			error = null;
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	}

	const KINDS = [
		['hosts', 'Host'],
		['services', 'Service'],
		['hostgroups', 'Host Group'],
		['servicegroups', 'Service Group'],
		['downtimegroups', 'Downtime Group']
	] as const;
	type TargetKey = (typeof KINDS)[number][0];

	let kind = $state<TargetKey>('hosts');
	let names = $state('');
	let start = $state('');
	let end = $state('');
	let length = $state('2h');
	let comment = $state('');

	let options = $derived.by(() => {
		switch (kind) {
			case 'hosts':
				return live.hosts.map((h) => h.name);
			case 'services':
				return live.services.map((s) => `${s.host}/${s.service}`);
			case 'hostgroups':
				return (groups?.hostgroups ?? []).map((g) => ({ value: g.name, hint: g.title ?? undefined }));
			case 'servicegroups':
				return (groups?.servicegroups ?? []).map((g) => ({ value: g.name, hint: g.title ?? undefined }));
			case 'downtimegroups':
				return (groups?.downtimegroups ?? []).map((g) => ({ value: g.name, hint: g.title ?? undefined }));
		}
	});

	async function schedule(e: SubmitEvent) {
		e.preventDefault();
		const list = names
			.split(',')
			.map((s) => s.trim())
			.filter(Boolean);
		const body: ActionBody = { [kind]: list, comment };
		if (start) body.start = new Date(start).toISOString();
		if (end) body.end = new Date(end).toISOString();
		else body.duration = length;
		if (await actions.run('downtime', body)) {
			names = '';
			comment = '';
		}
	}

	async function cancel(d: Downtime) {
		if (d.id === null) return;
		try {
			await api.cancelDowntime(d.id);
			actions.say(`Downtime ended: ${d.kind} ${d.target}`);
			await load();
		} catch (e) {
			actions.say(e instanceof Error ? e.message : String(e), true);
		}
	}

	const UNDO = { ack: 'unack', mute: 'unmute', no_checks: 'enable-checks' } as const;
	const FLAG = { ack: 'Acknowledged', mute: 'Muted', no_checks: 'Checks off' } as const;

	async function clear(f: Flag) {
		const body: ActionBody = f.target.includes('/') ? { services: [f.target] } : { hosts: [f.target] };
		if (await actions.run(UNDO[f.kind], body)) await load();
	}

	function targetHref(kind: string, target: string): string | undefined {
		if (kind === 'host') return hostHref(target);
		if (kind === 'service') {
			const [h, ...s] = target.split('/');
			return serviceHref(h ?? '', s.join('/'));
		}
		if (kind === 'hostgroup') return `/hosts?group=${encodeURIComponent(target)}`;
		if (kind === 'servicegroup') return `/services?group=${encodeURIComponent(target)}`;
		return undefined;
	}

	function flagHref(target: string) {
		return targetHref(target.includes('/') ? 'service' : 'host', target);
	}

	const span = (d: Downtime) => (Date.parse(d.end) - Date.parse(d.start)) / 1000;
</script>

<div class="content">
	<div class="page-head"><h1>Downtimes</h1></div>
	{#if error}<p class="notice error">{error}</p>{/if}

	<section class="panel">
		<div class="panel-title">Schedule Downtime</div>
		<form class="schedule" onsubmit={schedule}>
			<label for="dt-kind">For</label>
			<div class="row">
				<select id="dt-kind" bind:value={kind} onchange={() => (names = '')}>
					{#each KINDS as [key, title] (key)}<option value={key}>{title}</option>{/each}
				</select>
				<Combobox id="dt-names" multi mono bind:value={names} options={options} placeholder="names, comma-separated" required label="Names" />
			</div>
			<label for="dt-start">Start</label>
			<div class="row">
				<input id="dt-start" type="datetime-local" bind:value={start} />
				<span class="muted">empty: now</span>
			</div>
			<label for="dt-end">End</label>
			<div class="row">
				<input id="dt-end" type="datetime-local" bind:value={end} />
				<span class="muted">or</span>
				<input class="mono short" bind:value={length} disabled={!!end} aria-label="Duration" />
			</div>
			<label for="dt-comment">Comment</label>
			<input id="dt-comment" bind:value={comment} />
			<span></span>
			<div><button type="submit" class="btn primary">Schedule</button></div>
		</form>
	</section>

	<section class="panel">
		<div class="panel-title">Downtimes <span class="muted">{downtimes.length}</span></div>
		{#if downtimes.length === 0}
			<div class="empty">No downtimes.</div>
		{:else}
			<table class="grid">
				<thead>
					<tr>
						<th>Target</th>
						<th>Status</th>
						<th>Start</th>
						<th>End</th>
						<th class="hide-narrow">Duration</th>
						<th class="hide-narrow">Covers</th>
						<th class="hide-narrow">Author</th>
						<th>Comment</th>
						<th></th>
					</tr>
				</thead>
				<tbody>
					{#each downtimes as d, i (i)}
						{@const href = targetHref(d.kind, d.target)}
						<tr>
							<td class="nowrap"><span class="muted">{d.kind}</span> {#if href}<a {href}>{d.target}</a>{:else}{d.target}{/if}</td>
							<td class="nowrap">
								{#if d.active}<span class="tag downtime">ACTIVE</span>{:else}<span class="tag">SCHEDULED</span>{/if}
								{#if d.window}<span class="tag" title="Recurring, from the config">{d.window}</span>{/if}
							</td>
							<td class="nowrap">{datetime(d.start)}</td>
							<td class="nowrap">{datetime(d.end)}</td>
							<td class="nowrap hide-narrow">{duration(span(d))}</td>
							<td class="nowrap hide-narrow">{d.hosts} hosts, {d.services} services</td>
							<td class="hide-narrow">{d.author ?? 'config'}</td>
							<td class="info">{d.comment}</td>
							<td class="nowrap">
								{#if d.id !== null}<button type="button" class="btn small" onclick={() => cancel(d)}>{d.active ? 'End' : 'Cancel'}</button>{/if}
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		{/if}
	</section>

	<section class="panel">
		<div class="panel-title">Acknowledged, Muted, Checks Off <span class="muted">{flags.length}</span></div>
		{#if flags.length === 0}
			<div class="empty">None.</div>
		{:else}
			<table class="grid">
				<thead>
					<tr><th>Target</th><th>Type</th><th class="hide-narrow">Since</th><th class="hide-narrow">Author</th><th>Comment</th><th></th></tr>
				</thead>
				<tbody>
					{#each flags as f (`${f.kind}/${f.target}`)}
						<tr>
							<td class="nowrap"><a href={flagHref(f.target)}>{f.target}</a></td>
							<td class="nowrap">{FLAG[f.kind]}</td>
							<td class="nowrap hide-narrow">{datetime(f.created)}</td>
							<td class="hide-narrow">{f.author}</td>
							<td class="info">{f.comment}</td>
							<td class="nowrap"><button type="button" class="btn small" onclick={() => clear(f)}>Remove</button></td>
						</tr>
					{/each}
				</tbody>
			</table>
		{/if}
	</section>
</div>

<style>
	.schedule {
		display: grid;
		grid-template-columns: 110px minmax(0, 1fr);
		gap: 8px 14px;
		align-items: center;
		padding: 12px;
		max-width: 760px;
	}

	.schedule label {
		color: var(--muted);
		font-weight: 500;
	}

	.row {
		display: flex;
		gap: 8px;
		align-items: center;
		min-width: 0;
	}

	.row > :global(.combo) {
		flex: 1;
	}

	.short {
		width: 80px;
	}

	.btn.small {
		font-size: 12px;
		padding: 2px 8px;
	}

	.error {
		color: var(--crit);
	}
</style>
