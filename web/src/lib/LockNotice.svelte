<!-- Who else is editing a config file, with "Edit Anyway" while the page can't. -->
<script lang="ts">
	import type { Lock } from './api.ts';
	import { auth } from './auth.svelte.ts';
	import { datetime } from './format.ts';
	import type { FileLease } from './lease.svelte.ts';

	let { lease }: { lease: FileLease } = $props();

	const VIA: Record<Lock['via'], string> = {
		web: 'in the web UI',
		cli: 'with watcher config edit',
		vim: 'in vim',
		nano: 'in nano',
		emacs: 'in emacs',
		kate: 'in Kate'
	};

	let me = $derived(auth.status?.user?.username ?? 'local user');

	function who(l: Lock) {
		const by = l.by || 'Someone';
		const at = l.host && l.via !== 'web' ? ` on ${l.host}` : '';
		const since = l.since ? ` since ${datetime(l.since)}` : '';
		return `${by} is editing ${l.path} ${VIA[l.via]}${at}${since}.`;
	}
</script>

{#if lease.holder}
	{@const h = lease.holder}
	<div class="notice locked" role="alert">
		<strong>
			{#if h.via === 'web' && h.by === me && !lease.lost}
				Open in another window or tab.
			{:else if lease.lost}
				{h.by || 'Someone'} took over editing.
			{:else}
				{who(h)}
			{/if}
		</strong>
		Read-only until the file is free{h.via === 'web' ? '' : ' (when the editor closes it)'}.
		<button type="button" class="btn" onclick={() => lease.takeOver()}>Edit Anyway</button>
	</div>
{:else if lease.others.length > 0}
	<div class="notice also" role="status">
		{#each lease.others as o, i (i)}{who(o)}{' '}{/each}
		Whoever saves last wins; the other's changes are lost.
	</div>
{/if}
{#if lease.error}<p class="notice">Edit lock: {lease.error}</p>{/if}

<style>
	.notice {
		margin: 0 0 10px;
	}

	.locked,
	.also {
		border-color: var(--unknown);
	}

	.locked .btn {
		margin-left: 6px;
	}
</style>
