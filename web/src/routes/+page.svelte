<script lang="ts">
	import { serviceHref, type ServiceView } from '#lib/api.ts';
	import { age, severity } from '#lib/format.ts';
	import { live } from '#lib/live.svelte.ts';
	import Lamp from '#lib/Lamp.svelte';
	import Window from '#lib/Window.svelte';

	/** Worst first; confirmed before unconfirmed; newest change first. */
	function order(a: ServiceView, b: ServiceView) {
		return (
			severity(b.state) - severity(a.state) ||
			Number(b.state_type === 'hard') - Number(a.state_type === 'hard') ||
			Date.parse(b.last_change ?? '') - Date.parse(a.last_change ?? '')
		);
	}

	let problems = $derived(
		live.services.filter((s) => s.state !== null && s.state !== 'ok').sort(order)
	);

	let counts = $derived.by(() => {
		const c = { critical: 0, warning: 0, unknown: 0, pending: 0, ok: 0 };
		for (const s of live.services) c[s.state ?? 'pending']++;
		return c;
	});

	const TOTALS = [
		['critical', 'Critical'],
		['warning', 'Warning'],
		['unknown', 'Unknown'],
		['pending', 'Pending'],
		['ok', 'OK']
	] as const;

	function detail(s: ServiceView) {
		const since = age(s.last_change, live.now);
		return s.state_type === 'soft' ? `${since} · try ${s.attempt}/${s.max_attempts}` : since;
	}
</script>

<main class="page">
	<h1 class="visually-hidden">Problems</h1>

	<div class="totals" aria-label="Services by state">
		{#each TOTALS as [key, name] (key)}
			<a
				class="total plate"
				href={`/services?state=${key}`}
				data-state={key}
				class:zero={counts[key] === 0}
			>
				<span class="data number">{counts[key]}</span>
				<span class="legend">{name}</span>
			</a>
		{/each}
	</div>

	{#if !live.loaded}
		<p class="notice">Loading…</p>
	{:else if live.services.length === 0}
		<p class="notice">
			No services are configured yet. Add hosts and checks to your <code>conf.d</code> directory, then
			reload Watcher.
		</p>
	{:else if problems.length === 0}
		<div class="all-clear plate">
			<span class="legend">All clear</span>
			<p>
				{counts.ok} of {live.services.length} services are OK{#if counts.pending > 0}; {counts.pending}
					are waiting for their first result{/if}.
			</p>
		</div>
	{:else}
		<h2 class="section-label legend">Tripped</h2>
		<div class="panel">
			{#each problems as s (`${s.host}/${s.service}`)}
				<Window
					href={serviceHref(s.host, s.service)}
					title={s.host}
					caption={s.service}
					state={s.state}
					stateType={s.state_type}
					lastChange={s.last_change}
					now={live.now}
					detail={detail(s)}
				/>
			{/each}
		</div>

		<h2 class="section-label legend">What the checks report</h2>
		<table class="list">
			<thead>
				<tr>
					<th>State</th>
					<th>Service</th>
					<th class="hide-narrow">Since</th>
					<th>Output</th>
				</tr>
			</thead>
			<tbody>
				{#each problems as s (`${s.host}/${s.service}`)}
					<tr>
						<td><Lamp state={s.state} stateType={s.state_type} /></td>
						<td><a href={serviceHref(s.host, s.service)}>{s.host}/{s.service}</a></td>
						<td class="hide-narrow data">{age(s.last_change, live.now)}</td>
						<td class="output">{s.output}</td>
					</tr>
				{/each}
			</tbody>
		</table>
	{/if}
</main>

<style>
	.visually-hidden {
		position: absolute;
		width: 1px;
		height: 1px;
		overflow: hidden;
		clip: rect(0 0 0 0);
	}

	.totals {
		display: grid;
		grid-template-columns: repeat(5, minmax(0, 1fr));
		gap: 10px;
	}

	.total {
		display: flex;
		flex-direction: column;
		gap: 2px;
		padding: 12px 14px 10px;
		text-decoration: none;
		border-top: 4px solid var(--c, var(--rule));
	}

	.total:hover {
		border-color: var(--ink-3);
		border-top-color: var(--c, var(--rule));
	}

	.total[data-state='critical'] {
		--c: var(--crit);
	}
	.total[data-state='warning'] {
		--c: var(--warn);
	}
	.total[data-state='unknown'] {
		--c: var(--unknown);
	}
	.total[data-state='ok'] {
		--c: var(--ok);
	}
	.total[data-state='pending'] {
		--c: var(--pending);
	}

	.total.zero {
		--c: var(--rule);
		color: var(--ink-3);
	}

	.number {
		font-size: 30px;
		font-weight: 500;
		line-height: 1.1;
	}

	.total .legend {
		font-size: 12px;
		color: var(--ink-2);
	}

	/* The annunciator panel itself. */
	.panel {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(190px, 1fr));
		gap: 14px;
		padding: 18px;
		background: var(--plate-sunk);
		border: 1px solid var(--rule);
		border-radius: var(--radius);
		box-shadow: inset 0 2px 6px rgb(0 0 0 / 0.08);
	}

	.all-clear {
		margin-top: 28px;
		padding: 28px;
		text-align: center;
	}

	.all-clear .legend {
		font-size: 22px;
		color: var(--ok);
		letter-spacing: 0.18em;
	}

	.all-clear p {
		margin: 8px 0 0;
		color: var(--ink-2);
	}

	@media (max-width: 640px) {
		.totals {
			grid-template-columns: repeat(3, minmax(0, 1fr));
		}
		.number {
			font-size: 24px;
		}
		.panel {
			grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
			padding: 12px;
		}
	}
</style>
