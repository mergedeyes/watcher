<!--
	A text input with suggestions: typing opens a list of matching options,
	but any value can be typed. With `multi`, the value is comma-separated
	and suggestions complete the part after the last comma.
-->
<script lang="ts">
	import type { Option } from './suggest.ts';

	interface Props {
		value: string;
		options: (Option | string)[];
		multi?: boolean;
		id?: string;
		mono?: boolean;
		placeholder?: string;
		required?: boolean;
		label?: string;
		inputmode?: 'numeric';
	}

	let { value = $bindable(''), options, multi = false, id, mono = false, placeholder, required, label, inputmode }: Props = $props();

	const MAX = 50;
	const uid = `cb-${Math.random().toString(36).slice(2, 9)}`;

	let open = $state(false);
	let active = $state(-1);
	let input: HTMLInputElement;

	let list = $derived(options.map((o) => (typeof o === 'string' ? { value: o } : o)));

	/** The part being typed: everything, or (multi) after the last comma. */
	let token = $derived(multi ? (value.split(',').at(-1) ?? '').trim() : value.trim());
	let chosen = $derived(
		multi
			? value
					.split(',')
					.slice(0, -1)
					.map((s) => s.trim())
					.filter(Boolean)
			: []
	);

	let matches = $derived.by(() => {
		const q = token.toLowerCase();
		const free = list.filter((o) => !chosen.includes(o.value) && (multi || o.value !== value));
		if (!q) return free.slice(0, MAX);
		const starts = free.filter((o) => o.value.toLowerCase().startsWith(q));
		const contains = free.filter((o) => !o.value.toLowerCase().startsWith(q) && o.value.toLowerCase().includes(q));
		return [...starts, ...contains].slice(0, MAX);
	});

	let shown = $derived(open && matches.length > 0);

	function pick(option: Option) {
		if (multi) {
			value = [...chosen, option.value].join(', ') + ', ';
		} else {
			value = option.value;
			open = false;
		}
		active = -1;
		input.focus();
	}

	function onkeydown(e: KeyboardEvent) {
		switch (e.key) {
			case 'ArrowDown':
				e.preventDefault();
				if (!open) open = true;
				active = Math.min(active + 1, matches.length - 1);
				break;
			case 'ArrowUp':
				e.preventDefault();
				active = Math.max(active - 1, -1);
				break;
			case 'Enter':
				if (shown && active >= 0 && matches[active]) {
					e.preventDefault();
					pick(matches[active]!);
				}
				break;
			case 'Escape':
				if (shown) {
					e.preventDefault();
					e.stopPropagation();
					open = false;
				}
				break;
			case 'Tab':
				open = false;
				break;
		}
	}

	function oninput() {
		open = true;
		active = -1;
	}

	/** Split `text` around the first match of the typed part, to mark it. */
	function parts(text: string): [string, string, string] {
		const i = token ? text.toLowerCase().indexOf(token.toLowerCase()) : -1;
		if (i < 0) return [text, '', ''];
		return [text.slice(0, i), text.slice(i, i + token.length), text.slice(i + token.length)];
	}
</script>

<div class="combo">
	<input
		bind:this={input}
		{id}
		type="text"
		class:mono
		{placeholder}
		{required}
		{inputmode}
		aria-label={label}
		role="combobox"
		autocomplete="off"
		aria-autocomplete="list"
		aria-expanded={shown}
		aria-controls="{uid}-list"
		aria-activedescendant={shown && active >= 0 ? `${uid}-${active}` : undefined}
		bind:value
		{oninput}
		{onkeydown}
		onfocus={() => (open = true)}
		onblur={() => (open = false)}
	/>
	{#if list.length > 0}
		<button
			type="button"
			class="toggle"
			tabindex="-1"
			aria-label="Show options"
			onmousedown={(e) => {
				e.preventDefault();
				open = !open;
				input.focus();
			}}>▾</button
		>
	{/if}
	{#if shown}
		<ul class="options" id="{uid}-list" role="listbox">
			{#each matches as o, i (o.value)}
				{@const [a, b, c] = parts(o.value)}
				<li
					id="{uid}-{i}"
					role="option"
					aria-selected={i === active}
					class:active={i === active}
					onmousedown={(e) => {
						e.preventDefault();
						pick(o);
					}}
					onmouseenter={() => (active = i)}
				>
					<span class:mono>{a}<mark>{b}</mark>{c}</span>
					{#if o.hint}<span class="hint">{o.hint}</span>{/if}
				</li>
			{/each}
		</ul>
	{/if}
</div>

<style>
	.combo {
		position: relative;
		display: flex;
		width: 100%;
		min-width: 0;
	}

	input {
		flex: 1;
		min-width: 0;
		padding-right: 22px;
	}

	.toggle {
		position: absolute;
		right: 2px;
		top: 50%;
		transform: translateY(-50%);
		border: 0;
		background: none;
		color: var(--muted);
		cursor: pointer;
		font-size: 11px;
		padding: 2px 5px;
	}

	.options {
		position: absolute;
		top: calc(100% + 2px);
		left: 0;
		right: 0;
		z-index: 30;
		max-height: 240px;
		overflow-y: auto;
		margin: 0;
		padding: 2px 0;
		list-style: none;
		background: var(--panel);
		border: 1px solid var(--line);
		border-radius: 3px;
		box-shadow: 0 4px 12px rgb(0 0 0 / 0.15);
		font-size: 13px;
	}

	li {
		display: flex;
		justify-content: space-between;
		gap: 12px;
		padding: 4px 8px;
		cursor: pointer;
	}

	li.active {
		background: color-mix(in srgb, var(--link) 14%, transparent);
	}

	mark {
		background: none;
		color: inherit;
		font-weight: 700;
	}

	.hint {
		color: var(--muted);
		font-size: 11px;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
</style>
