// Sortable tables: click a column to sort, again to reverse.

export type Compare<T> = (a: T, b: T) => number;

export class Sorter<K extends string> {
	key = $state<K>() as K;
	desc = $state(false);

	constructor(key: K, desc = false) {
		this.key = key;
		this.desc = desc;
	}

	toggle(key: K) {
		if (this.key === key) this.desc = !this.desc;
		else {
			this.key = key;
			this.desc = false;
		}
	}

	/** Sorted copy; ties are broken by the following comparators in `by`. */
	apply<T>(rows: T[], by: Record<K, Compare<T>>): T[] {
		const primary = by[this.key];
		const sign = this.desc ? -1 : 1;
		const rest = Object.values(by) as Compare<T>[];
		return [...rows].sort((a, b) => {
			const first = sign * primary(a, b);
			if (first !== 0) return first;
			for (const c of rest) {
				const r = c(a, b);
				if (r !== 0) return r;
			}
			return 0;
		});
	}

	/** `▲`/`▼` for the active column. */
	mark(key: K): string {
		return this.key === key ? (this.desc ? ' ▼' : ' ▲') : '';
	}
}
