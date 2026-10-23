<!--
	The right-click menu of hosts and services, the dialog for commands
	that need a comment, time or result, and the result message. Mounted
	once in the layout.
-->
<script lang="ts">
	import { api, type State } from './api.ts';
	import { actions, body, entries, label } from './actions.svelte.ts';

	let dialog: HTMLDialogElement;
	let menu = $state<HTMLElement>();
	let comment = $state('');
	let duration = $state('2h');
	let start = $state('');
	let end = $state('');
	let resultState = $state<State>('ok');
	let output = $state('');
	let busy = $state(false);

	const DURATIONS = ['30m', '1h', '2h', '4h', '8h', '1d', '7d'];

	/** `YYYY-MM-DDTHH:MM` in local time, for datetime-local inputs. */
	function local(d: Date): string {
		const p = (n: number) => String(n).padStart(2, '0');
		return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(d.getHours())}:${p(d.getMinutes())}`;
	}

	$effect(() => {
		if (!actions.dialog) return;
		comment = '';
		duration = '2h';
		start = local(new Date());
		end = '';
		resultState = 'ok';
		output = '';
		dialog.showModal();
	});

	// Keep the menu inside the window.
	let position = $derived.by(() => {
		const m = actions.menu;
		if (!m) return '';
		const x = Math.min(m.x, window.innerWidth - 240);
		const y = Math.min(m.y, window.innerHeight - 220);
		return `left: ${Math.max(4, x)}px; top: ${Math.max(4, y)}px`;
	});

	function onclick(e: MouseEvent) {
		// A right-click on a row opened the menu (and prevented the default).
		if (e.type === 'contextmenu' && e.defaultPrevented) return;
		if (actions.menu && !menu?.contains(e.target as Node)) actions.menu = null;
	}

	function onkeydown(e: KeyboardEvent) {
		if (actions.menu && e.key === 'Escape') actions.menu = null;
	}

	async function submit(e: SubmitEvent) {
		e.preventDefault();
		const d = actions.dialog;
		if (!d) return;
		busy = true;
		let ok: boolean;
		if (d.entry.action === 'submit' && d.target.service) {
			try {
				await api.submit(d.target.service.host, d.target.service.service, resultState, output);
				actions.say('Result submitted');
				ok = true;
			} catch (err) {
				actions.say(err instanceof Error ? err.message : String(err), true);
				ok = false;
			}
		} else if (d.entry.action === 'downtime') {
			const b = { ...body(d.target), comment, start: new Date(start).toISOString() };
			ok = await actions.run('downtime', end ? { ...b, end: new Date(end).toISOString() } : { ...b, duration });
		} else if (d.entry.action !== 'submit') {
			ok = await actions.run(d.entry.action, { ...body(d.target), comment });
		} else ok = false;
		busy = false;
		if (ok) dialog.close();
	}
</script>

<svelte:window {onclick} {onkeydown} oncontextmenu={onclick} />

{#if actions.menu}
	{@const target = actions.menu.target}
	<div class="menu" role="menu" style={position} bind:this={menu}>
		<div class="title mono">{label(target)}</div>
		{#each entries(target) as entry (entry.label)}
			<button type="button" role="menuitem" onclick={() => actions.choose(entry, target)}>
				{entry.label}{entry.dialog ? '…' : ''}
			</button>
		{/each}
	</div>
{/if}

<dialog bind:this={dialog} onclose={() => (actions.dialog = null)}>
	{#if actions.dialog}
		{@const d = actions.dialog}
		<form onsubmit={submit}>
			<div class="head">{d.entry.label} <span class="muted mono">{label(d.target)}</span></div>
			<div class="body">
				{#if d.entry.action === 'submit'}
					<label for="a-state">State</label>
					<select id="a-state" bind:value={resultState}>
						<option value="ok">OK</option>
						<option value="warning">WARNING</option>
						<option value="critical">CRITICAL</option>
						<option value="unknown">UNKNOWN</option>
					</select>
					<label for="a-output">Output</label>
					<input id="a-output" class="mono" bind:value={output} placeholder="BACKUP OK | size=12GB" />
				{:else}
					{#if d.entry.action === 'downtime'}
						<label for="a-start">Start</label>
						<input id="a-start" type="datetime-local" bind:value={start} required />
						<label for="a-end">End</label>
						<input id="a-end" type="datetime-local" bind:value={end} />
						<label for="a-duration">Duration</label>
						<div class="durations">
							{#each DURATIONS as dur (dur)}
								<button type="button" class="btn small" class:on={!end && duration === dur} onclick={() => ((duration = dur), (end = ''))}>{dur}</button>
							{/each}
							<input id="a-duration" class="mono" bind:value={duration} disabled={!!end} aria-label="Duration" />
						</div>
					{/if}
					<label for="a-comment">Comment</label>
					<input id="a-comment" bind:value={comment} placeholder={d.entry.action === 'ack' ? 'on it' : ''} />
				{/if}
			</div>
			<div class="foot">
				<button type="button" class="btn" onclick={() => dialog.close()}>Cancel</button>
				<button type="submit" class="btn primary" disabled={busy}>{busy ? '…' : 'OK'}</button>
			</div>
		</form>
	{/if}
</dialog>

{#if actions.message}
	<div class="toast" class:error={actions.message.error} role="status">{actions.message.text}</div>
{/if}

<style>
	.menu {
		position: fixed;
		z-index: 50;
		min-width: 220px;
		padding: 2px 0;
		background: var(--panel);
		border: 1px solid var(--line);
		border-radius: 3px;
		box-shadow: 0 4px 12px rgb(0 0 0 / 0.2);
		font-size: 13px;
	}

	.menu .title {
		padding: 5px 10px;
		font-size: 11px;
		color: var(--muted);
		border-bottom: 1px solid var(--line);
	}

	.menu button {
		display: block;
		width: 100%;
		padding: 6px 10px;
		border: 0;
		background: none;
		color: var(--text);
		text-align: left;
		font: inherit;
		cursor: pointer;
	}

	.menu button:hover,
	.menu button:focus-visible {
		background: color-mix(in srgb, var(--link) 14%, transparent);
	}

	dialog {
		width: min(460px, calc(100vw - 32px));
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

	.head .muted {
		font-weight: 400;
		margin-left: 6px;
	}

	.body {
		display: grid;
		grid-template-columns: 90px minmax(0, 1fr);
		gap: 8px 10px;
		align-items: center;
		padding: 12px 14px;
	}

	.body label {
		color: var(--muted);
		font-weight: 500;
	}

	.durations {
		display: flex;
		flex-wrap: wrap;
		gap: 4px;
		align-items: center;
	}

	.durations input {
		width: 70px;
	}

	.btn.small {
		font-size: 12px;
		padding: 2px 7px;
	}

	.btn.on {
		border-color: var(--link);
		color: var(--link);
	}

	.foot {
		display: flex;
		justify-content: flex-end;
		gap: 8px;
		padding: 10px 14px;
		border-top: 1px solid var(--line);
	}

	.toast {
		position: fixed;
		right: 16px;
		bottom: 16px;
		z-index: 60;
		max-width: min(520px, calc(100vw - 32px));
		padding: 8px 12px;
		border-radius: var(--radius);
		background: var(--panel);
		border: 1px solid var(--line);
		border-left: 4px solid var(--ok);
		box-shadow: 0 4px 12px rgb(0 0 0 / 0.2);
		font-size: 13px;
	}

	.toast.error {
		border-left-color: var(--crit);
	}
</style>
