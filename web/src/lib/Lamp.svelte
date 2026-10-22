<!-- A small indicator lamp with the state name, for tables and headers. -->
<script lang="ts">
	import type { State, StateType } from './api.ts';
	import { stateName } from './format.ts';

	interface Props {
		state: State | null;
		stateType?: StateType | null;
		/** Show the state name next to the lamp. */
		label?: boolean;
	}

	let { state, stateType = null, label = true }: Props = $props();
</script>

<span class="lamp-wrap" title={stateType === 'soft' ? `${stateName(state)} (SOFT)` : stateName(state)}>
	<span class="lamp" data-state={state ?? 'pending'} class:soft={stateType === 'soft'}></span>
	{#if label}<span class="legend name">{stateName(state)}</span>{/if}
</span>

<style>
	.lamp-wrap {
		display: inline-flex;
		align-items: center;
		gap: 7px;
		white-space: nowrap;
	}

	.lamp {
		width: 10px;
		height: 10px;
		border-radius: 50%;
		flex: none;
		background: var(--c);
		box-shadow:
			0 0 0 1px rgb(0 0 0 / 0.2),
			0 0 6px -1px var(--c);
	}

	.lamp[data-state='ok'] {
		--c: var(--ok);
	}
	.lamp[data-state='warning'] {
		--c: var(--warn);
	}
	.lamp[data-state='critical'] {
		--c: var(--crit);
	}
	.lamp[data-state='unknown'] {
		--c: var(--unknown);
	}
	.lamp[data-state='pending'] {
		--c: transparent;
		box-shadow: inset 0 0 0 1.5px var(--pending);
	}

	/* Unconfirmed: a ring instead of a solid lamp. */
	.lamp.soft {
		background: transparent;
		box-shadow:
			inset 0 0 0 2.5px var(--c),
			0 0 6px -1px var(--c);
	}

	.name {
		font-size: 12px;
	}
</style>
