<!-- What a selector or member list matches in the running config: "12 hosts: web01 (name web*), …". -->
<script lang="ts">
	import { api, hostHref, serviceHref } from './api.ts';

	interface Props {
		kind: 'host' | 'service';
		/** Selectors as written; a match of any counts. Empty: nothing to show. */
		selectors: unknown[];
		/** Text when a selector is absent, e.g. "all hosts" for notify rules. */
		none?: string;
	}

	let { kind, selectors, none }: Props = $props();

	let result = $state<{ total: number; matches: { name: string; why: string }[] } | null>(null);
	let error = $state<string | null>(null);
	let all = $state(false);
	const SHOWN = 12;

	$effect(() => {
		const json = JSON.stringify(selectors);
		if (selectors.length === 0) {
			result = null;
			error = null;
			return;
		}
		// Wait for typing to pause.
		const timer = setTimeout(() => {
			api.match(kind, JSON.parse(json)).then(
				(r) => {
					result = r;
					error = null;
				},
				(e) => {
					result = null;
					error = e instanceof Error ? e.message : String(e);
				}
			);
		}, 300);
		return () => clearTimeout(timer);
	});

	function href(name: string) {
		if (kind === 'host') return hostHref(name);
		const [h, ...s] = name.split('/');
		return serviceHref(h ?? '', s.join('/'));
	}

	let shown = $derived(result ? (all ? result.matches : result.matches.slice(0, SHOWN)) : []);
	let unit = $derived(kind === 'host' ? 'host' : 'service');
</script>

<div class="preview">
	{#if error}
		<span class="error">{error}</span>
	{:else if selectors.length === 0}
		{#if none}<span class="muted">{none}</span>{/if}
	{:else if result}
		<span class="count">{result.total === 0 ? `No ${unit}s` : `${result.total} ${unit}${result.total === 1 ? '' : 's'}`}</span>{#if result.total > 0}:{/if}
		{#each shown as m, i (m.name)}
			{i ? ', ' : ' '}<a href={href(m.name)} title={m.why}>{m.name}</a>{#if m.why !== 'listed' && !m.why.startsWith('all')}<span class="why"> ({m.why})</span>{/if}
		{/each}
		{#if result.matches.length > SHOWN}
			<button type="button" class="more" onclick={() => (all = !all)}>{all ? 'less' : `+${result.total - SHOWN} more`}</button>
		{/if}
	{/if}
</div>

<style>
	.preview {
		margin-top: 4px;
		font-size: 12px;
		line-height: 1.5;
	}

	.count {
		font-weight: 600;
	}

	.why,
	.muted {
		color: var(--muted);
	}

	.error {
		color: var(--crit);
	}

	.more {
		margin-left: 6px;
		border: 0;
		padding: 0;
		background: none;
		color: var(--link);
		font: inherit;
		cursor: pointer;
	}
</style>
