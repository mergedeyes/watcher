// Lists fetched from the server a page at a time. A list refetches when
// its parameters change, after reloads, and shortly after states changed
// (live.generation); in between, watched results patch its rows in place.
// Create it while a component initializes: it runs effects.

import { untrack } from 'svelte';
import type { HostView, ServiceView, Watch } from './api.ts';
import { live } from './live.svelte.ts';

/** Wait after a state change before refetching, so bursts fetch once. */
const SETTLE_MS = 300;

export interface ListOptions<R> {
	/** What to get results of for the fetched data. */
	watch?: (data: R) => Watch;
	/** Applies a result to the data (a deep $state proxy). */
	patch?: (data: R, view: ServiceView) => void;
}

export class ServerList<P, R> {
	data = $state<R | null>(null);
	error = $state<string | null>(null);
	#seq = 0;

	constructor(params: () => P, load: (p: P) => Promise<R>, options: ListOptions<R> = {}) {
		const fetch = async (p: P) => {
			const seq = ++this.#seq;
			try {
				const data = await load(p);
				if (seq !== this.#seq) return;
				this.data = data;
				this.error = null;
			} catch (e) {
				if (seq === this.#seq) this.error = e instanceof Error ? e.message : String(e);
			}
		};

		$effect(() => {
			const p = params();
			void live.reloads;
			untrack(() => fetch(p));
		});

		let first = true;
		$effect(() => {
			void live.generation;
			if (first) {
				first = false;
				return;
			}
			const later = setTimeout(() => fetch(untrack(params)), SETTLE_MS);
			return () => clearTimeout(later);
		});

		$effect(() => {
			const data = this.data;
			const { watch, patch } = options;
			if (!data || !watch) return;
			return live.watch(
				untrack(() => watch(data)),
				(view) => patch?.(data, view)
			);
		});
	}
}

/** `host/service`. */
export function sid(s: { host: string; service: string }): string {
	return `${s.host}/${s.service}`;
}

/** Replaces the row of `view` in `rows`, if it is there. */
export function patchRow(rows: ServiceView[], view: ServiceView) {
	const id = sid(view);
	const i = rows.findIndex((s) => sid(s) === id);
	if (i >= 0) rows[i] = view;
}

/** The host check's id, if the host has one. */
export function checkId(host: HostView): string | null {
	return host.check ? `${host.name}/${host.check}` : null;
}

/** Host state from a new host check result: OK/WARNING is UP (confirmed), else DOWN. */
export function patchHost(host: HostView, check: ServiceView) {
	if (check.host !== host.name || check.service !== host.check) return;
	const up = check.state === 'ok' || check.state === 'warning';
	host.state = check.state === null ? null : up ? 'up' : 'down';
	host.state_type = up ? 'hard' : check.state_type;
	host.attempt = check.attempt;
	host.max_attempts = check.max_attempts;
	host.last_check = check.last_check;
	host.last_change = check.last_change;
	host.output = check.output;
	host.flapping = check.flapping;
}

/** A text input's value, settled for `ms` (for searches). */
export class Debounced {
	value = $state('');
	settled = $state('');
	#timer: ReturnType<typeof setTimeout> | undefined;

	constructor(ms = 250) {
		$effect(() => {
			const v = this.value;
			clearTimeout(this.#timer);
			this.#timer = setTimeout(() => (this.settled = v.trim()), v ? ms : 0);
			return () => clearTimeout(this.#timer);
		});
	}
}

/** Rows per page: the last choice, kept in this browser. */
export const PAGE_SIZES = [50, 100, 250, 500] as const;
const SIZE_KEY = 'watcher.pageSize';

export function savedPageSize(): number {
	try {
		const n = Number(localStorage.getItem(SIZE_KEY));
		if ((PAGE_SIZES as readonly number[]).includes(n)) return n;
	} catch {
		// storage unavailable
	}
	return 100;
}

export function savePageSize(n: number) {
	try {
		localStorage.setItem(SIZE_KEY, String(n));
	} catch {
		// storage unavailable
	}
}

/**
 * The offset of a paged list, back to the first page whenever `key`
 * (filters, sort) or the page size changes.
 */
export class Paging {
	limit = $state(savedPageSize());
	#at = $state({ key: '', offset: 0 });
	#key: () => string;

	constructor(key: () => string) {
		this.#key = () => `${key()}|${this.limit}`;
	}

	get offset(): number {
		const key = this.#key();
		return this.#at.key === key ? this.#at.offset : 0;
	}

	go(offset: number) {
		this.#at = { key: this.#key(), offset: Math.max(0, offset) };
	}

	setLimit(n: number) {
		this.limit = n;
		savePageSize(n);
	}
}
