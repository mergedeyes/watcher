<!--
	One annunciator window: dark while normal, lit in the state colour when
	tripped. Hatching marks an unconfirmed (SOFT) state; a slow pulse marks
	one that tripped in the last ten minutes. Pending windows are outlined.
-->
<script lang="ts">
	import type { State, StateType } from './api.ts';
	import { stateName } from './format.ts';

	interface Props {
		href: string;
		/** First legend line (e.g. the host). */
		title: string;
		/** Second legend line (e.g. the service). */
		caption?: string;
		state: State | null;
		stateType?: StateType | null;
		lastChange?: string | null;
		now: number;
		/** Small text at the bottom, e.g. "12m · SOFT 2/3". */
		detail?: string;
	}

	let { href, title, caption, state, stateType = null, lastChange = null, now, detail }: Props =
		$props();

	const RECENT_MS = 10 * 60 * 1000;
	let lit = $derived(state !== null && state !== 'ok');
	let recent = $derived(
		lit && lastChange !== null && now - Date.parse(lastChange) < RECENT_MS
	);
	let label = $derived(
		[title, caption, stateName(state), stateType === 'soft' ? 'SOFT' : null]
			.filter(Boolean)
			.join(', ')
	);
</script>

<a
	{href}
	class="window"
	class:lit
	class:soft={lit && stateType === 'soft'}
	class:recent
	class:pending={state === null}
	data-state={state ?? 'pending'}
	aria-label={label}
>
	<span class="legend title">{title}</span>
	{#if caption}<span class="legend caption">{caption}</span>{/if}
	{#if detail}<span class="detail data">{detail}</span>{/if}
</a>

<style>
	.window {
		--lamp: var(--glass);
		--lamp-ink: var(--glass-ink);
		position: relative;
		display: flex;
		flex-direction: column;
		justify-content: center;
		gap: 2px;
		min-height: 92px;
		padding: 12px 14px;
		border-radius: 2px;
		text-decoration: none;
		text-align: center;
		color: var(--lamp-ink);
		background: var(--lamp);
		/* The bezel around the glass. */
		box-shadow:
			0 0 0 1px rgb(0 0 0 / 0.18),
			0 0 0 4px var(--plate-sunk),
			0 0 0 5px var(--rule),
			inset 0 1px 0 rgb(255 255 255 / 0.25),
			inset 0 -10px 18px rgb(0 0 0 / 0.08);
		transition: filter 120ms ease;
	}

	.window:hover {
		filter: brightness(1.05);
	}

	.window[data-state='critical'] {
		--lamp: var(--crit);
		--lamp-ink: var(--on-crit);
	}
	.window[data-state='warning'] {
		--lamp: var(--warn);
		--lamp-ink: var(--on-warn);
	}
	.window[data-state='unknown'] {
		--lamp: var(--unknown);
		--lamp-ink: var(--on-unknown);
	}
	/* An OK window stays dark: nothing to read there. */
	.window[data-state='ok'] {
		--lamp: var(--glass);
		--lamp-ink: var(--glass-ink);
	}

	/* Backlit frosted glass: brightest in the middle. */
	.lit {
		background:
			radial-gradient(
				ellipse 70% 80% at 50% 45%,
				color-mix(in srgb, var(--lamp) 60%, white) 0%,
				var(--lamp) 75%
			);
		box-shadow:
			0 0 0 1px rgb(0 0 0 / 0.25),
			0 0 0 4px var(--plate-sunk),
			0 0 0 5px var(--rule),
			0 0 22px -4px color-mix(in srgb, var(--lamp) 70%, transparent),
			inset 0 -10px 18px rgb(0 0 0 / 0.12);
	}

	/* Unconfirmed: hatched glass. */
	.soft {
		background:
			repeating-linear-gradient(
				135deg,
				transparent 0 7px,
				rgb(255 255 255 / 0.28) 7px 10px
			),
			radial-gradient(
				ellipse 70% 80% at 50% 45%,
				color-mix(in srgb, var(--lamp) 45%, white) 0%,
				color-mix(in srgb, var(--lamp) 80%, white) 75%
			);
	}

	.pending {
		background: transparent;
		color: var(--ink-3);
		box-shadow: none;
		outline: 1px dashed var(--rule);
		outline-offset: -1px;
	}

	.recent {
		animation: trip 1.6s ease-in-out infinite;
	}

	@keyframes trip {
		50% {
			filter: brightness(1.22) saturate(1.1);
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.recent {
			animation: none;
		}
	}

	.title {
		font-size: 15px;
		line-height: 1.15;
		overflow-wrap: anywhere;
	}

	.caption {
		font-size: 13px;
		line-height: 1.15;
		opacity: 0.85;
		overflow-wrap: anywhere;
	}

	.detail {
		margin-top: 6px;
		font-size: 11px;
		opacity: 0.8;
	}

	@media (max-width: 640px) {
		.title {
			font-size: 14px;
		}
		.caption {
			font-size: 12px;
			letter-spacing: 0.03em;
		}
	}

	.window:focus-visible {
		outline: 2px solid var(--focus);
		outline-offset: 7px;
	}
</style>
