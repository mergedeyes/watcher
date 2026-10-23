<script lang="ts">
	import { api, type ProcessInfo } from '#lib/api.ts';
	import { actions } from '#lib/actions.svelte.ts';
	import { auth } from '#lib/auth.svelte.ts';
	import { datetime, since } from '#lib/format.ts';
	import { live } from '#lib/live.svelte.ts';

	let info = $state<ProcessInfo | null>(null);
	let error = $state<string | null>(null);

	$effect(() => {
		void live.reloads;
		void live.now;
		load();
	});

	async function load() {
		try {
			info = await api.process();
			error = null;
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	}

	async function toggle(name: 'notifications' | 'checks', on: boolean) {
		try {
			await api.setProcess({ [name]: on });
			actions.say(`${name === 'checks' ? 'Active checks' : 'Notifications'} ${on ? 'on' : 'off'}`);
			await load();
		} catch (e) {
			actions.say(e instanceof Error ? e.message : String(e), true);
		}
	}
</script>

<div class="content">
	<div class="page-head"><h1>Process Info</h1></div>
	{#if error}<p class="notice error">{error}</p>{/if}
	{#if info}
		<div class="cols">
			<section class="panel">
				<div class="panel-title">Process</div>
				<table class="grid kv">
					<tbody>
						<tr><th>Version</th><td>{info.version}</td></tr>
						{#if info.instance}<tr><th>Instance</th><td>{info.instance}</td></tr>{/if}
						<tr><th>PID</th><td class="mono">{info.pid}</td></tr>
						<tr><th>Started</th><td>{datetime(info.started)}</td></tr>
						<tr><th>Running Time</th><td>{since(info.started, live.now)}</td></tr>
						<tr><th>Config Loaded</th><td>{datetime(info.config_loaded)}</td></tr>
						<tr><th>Database</th><td>{info.database ?? 'none'}</td></tr>
						<tr><th>Hosts / Services</th><td>{info.hosts} / {info.services}</td></tr>
					</tbody>
				</table>
			</section>

			<section class="panel">
				<div class="panel-title">Process Commands</div>
				<table class="grid kv">
					<tbody>
						<tr>
							<th>Notifications</th>
							<td>
								<span class="tag" class:on={info.notifications} class:off={!info.notifications}>{info.notifications ? 'ENABLED' : 'DISABLED'}</span>
								{#if auth.admin}
									<button type="button" class="btn small" onclick={() => toggle('notifications', !info!.notifications)}>
										{info.notifications ? 'Disable' : 'Enable'}
									</button>
								{/if}
							</td>
						</tr>
						<tr>
							<th>Active Checks</th>
							<td>
								<span class="tag" class:on={info.checks} class:off={!info.checks}>{info.checks ? 'ENABLED' : 'DISABLED'}</span>
								{#if auth.admin}
									<button type="button" class="btn small" onclick={() => toggle('checks', !info!.checks)}>
										{info.checks ? 'Stop' : 'Start'}
									</button>
								{/if}
							</td>
						</tr>
					</tbody>
				</table>
			</section>

			<section class="panel">
				<div class="panel-title">Performance</div>
				<table class="grid kv">
					<tbody>
						<tr><th>Scheduled Checks</th><td>{info.scheduler.jobs - info.scheduler.passive} active, {info.scheduler.passive} passive</td></tr>
						<tr><th>Running Now</th><td>{info.scheduler.running}</td></tr>
						<tr><th>Checks Off</th><td>{info.scheduler.paused}</td></tr>
						<tr><th>Results</th><td>{info.results.last_1m} / {info.results.last_5m} / {info.results.last_15m} <span class="muted">last 1 / 5 / 15 min</span></td></tr>
					</tbody>
				</table>
			</section>

			<section class="panel">
				<div class="panel-title">Runtime</div>
				<table class="grid kv">
					<tbody>
						<tr><th>Acknowledged</th><td><a href="/downtimes">{info.acknowledged}</a></td></tr>
						<tr><th>Muted</th><td><a href="/downtimes">{info.muted}</a></td></tr>
						<tr><th>Checks Off</th><td><a href="/downtimes">{info.checks_off}</a> <span class="muted">hosts and services</span></td></tr>
						<tr><th>In Downtime</th><td><a href="/downtimes">{info.in_downtime}</a> <span class="muted">services</span></td></tr>
						<tr><th>Scheduling Queue</th><td><a href="/scheduler">show</a></td></tr>
					</tbody>
				</table>
			</section>
		</div>
	{:else if !error}
		<p class="notice">Loading…</p>
	{/if}
</div>

<style>
	.cols {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(380px, 1fr));
		gap: 16px;
		align-items: start;
	}

	.cols :global(section + section) {
		margin-top: 0;
	}

	.kv th {
		width: 170px;
	}

	.tag.on {
		color: var(--ok);
		border-color: var(--ok);
		font-weight: 600;
	}

	.tag.off {
		color: var(--crit);
		border-color: var(--crit);
		font-weight: 600;
	}

	.btn.small {
		margin-left: 8px;
		font-size: 12px;
		padding: 2px 8px;
	}

	.error {
		color: var(--crit);
	}
</style>
