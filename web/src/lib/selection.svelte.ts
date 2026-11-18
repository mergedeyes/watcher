// The rows ticked in a host or service table, for commands on several at
// once. Only rows on the current page count: rows that leave it (another
// page, a filter, a problem that recovered) are dropped from the
// selection, so a command never reaches a row the user can't see.

import { untrack } from 'svelte';
import { SvelteSet } from 'svelte/reactivity';
import { id, type Target } from './actions.svelte.ts';

export class Selection {
	#ids = new SvelteSet<string>();
	/** The rows shown, in table order; the table sets them. */
	#shown = $state.raw<Target[]>([]);
	/** The last row clicked, where a shift-click range starts. */
	#anchor: string | null = null;

	/** The selected rows, in table order and as last fetched. */
	targets = $derived(this.#shown.filter((t) => this.#ids.has(id(t))));
	all = $derived(this.#shown.length > 0 && this.targets.length === this.#shown.length);

	/** Sets the rows shown, dropping selected ones that are gone. */
	show(rows: Target[]) {
		this.#shown = rows;
		const ids = new Set(rows.map(id));
		untrack(() => {
			for (const x of [...this.#ids]) if (!ids.has(x)) this.#ids.delete(x);
		});
	}

	has(t: Target): boolean {
		return this.#ids.has(id(t));
	}

	/** Toggles a row; with shift, every row from the last one clicked to this one. */
	click(t: Target, shift: boolean) {
		const key = id(t);
		const on = !this.#ids.has(key);
		const order = this.#shown.map(id);
		const from = this.#anchor === null ? -1 : order.indexOf(this.#anchor);
		const to = order.indexOf(key);
		if (shift && from >= 0 && to >= 0) {
			for (const x of order.slice(Math.min(from, to), Math.max(from, to) + 1)) {
				if (on) this.#ids.add(x);
				else this.#ids.delete(x);
			}
		} else if (on) this.#ids.add(key);
		else this.#ids.delete(key);
		this.#anchor = key;
	}

	/** Selects every row shown, or none. */
	setAll(on: boolean) {
		this.#ids.clear();
		if (on) for (const t of this.#shown) this.#ids.add(id(t));
		this.#anchor = null;
	}

	clear() {
		this.setAll(false);
	}
}
