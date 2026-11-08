// Live status from the hub's event stream: the host and service totals,
// a generation that changes with every state change, and results of the
// rows pages show. Lists are fetched by the pages themselves, a page at a
// time (see list.svelte.ts); they refetch when `generation` or `reloads`
// changes and patch their rows from watched results in between.
//
// Paused, the stream is closed and nothing changes until refresh().

import { api, type ServiceView, type Summary, type Watch } from './api.ts';

export type Connection = 'connecting' | 'live' | 'offline' | 'paused';

interface Listener {
	services: Set<string>;
	hosts: Set<string>;
	onResult: (view: ServiceView) => void;
}

class Live {
	summary = $state<Summary | null>(null);
	connection = $state<Connection>('connecting');
	error = $state<string | null>(null);
	/** Ticks every 10 s, for ages like "12m". */
	now = $state(Date.now());
	/** Counts config reloads, commands, reconnects and refreshes: refetch everything. */
	reloads = $state(0);
	/** Changes with states, flags and the config: lists refetch their page. */
	generation = $state(0);

	#source: EventSource | null = null;
	#timer: ReturnType<typeof setInterval> | undefined;
	#started = false;
	/** Opened before: a new open is a reconnect. */
	#opened = false;
	#listeners = new Set<Listener>();
	/** The stream's id from its `hello`. */
	#stream: string | null = null;
	/** The watch last sent, to skip unchanged ones. */
	#sent = '';
	#syncing = false;

	get loaded() {
		return this.summary !== null;
	}

	start() {
		if (this.#started) return;
		this.#started = true;
		this.#timer = setInterval(() => (this.now = Date.now()), 10_000);
		if (this.connection === 'paused') this.#fetchSummary();
		else this.#connect();
	}

	/** Stops updates until resume(); refresh() fetches once. */
	pause() {
		this.#close();
		this.connection = 'paused';
	}

	resume() {
		if (this.connection !== 'paused') return;
		this.connection = 'connecting';
		if (this.#started) this.#connect();
	}

	async refresh() {
		this.now = Date.now();
		this.reloads++;
		await this.#fetchSummary();
	}

	/** Disconnects and forgets everything, e.g. after logging out. */
	stop() {
		this.#close();
		this.#started = false;
		this.#opened = false;
		clearInterval(this.#timer);
		this.summary = null;
		this.error = null;
		this.connection = 'connecting';
	}

	/**
	 * Calls `onResult` with every new result of the watched services (and
	 * all services of the watched hosts) until the returned function is
	 * called. Meant as the return value of an `$effect`.
	 */
	watch(watch: Watch, onResult: (view: ServiceView) => void): () => void {
		const listener: Listener = {
			services: new Set(watch.services ?? []),
			hosts: new Set(watch.hosts ?? []),
			onResult
		};
		this.#listeners.add(listener);
		this.#sync();
		return () => {
			this.#listeners.delete(listener);
			this.#sync();
		};
	}

	async #fetchSummary() {
		try {
			this.#setSummary(await api.summary());
			this.error = null;
		} catch (e) {
			this.error = e instanceof Error ? e.message : String(e);
		}
	}

	#setSummary(summary: Summary) {
		this.summary = summary;
		if (summary.generation !== this.generation) this.generation = summary.generation;
	}

	#close() {
		this.#source?.close();
		this.#source = null;
		this.#stream = null;
		this.#sent = '';
	}

	#connect() {
		const source = new EventSource('/api/v1/stream?watch=true');
		this.#source = source;
		source.onopen = () => {
			this.connection = 'live';
			this.error = null;
			// Events may have been missed while disconnected.
			if (this.#opened) this.reloads++;
			this.#opened = true;
		};
		// EventSource reconnects by itself, with a new stream id.
		source.onerror = () => {
			this.connection = 'offline';
			this.#stream = null;
			this.#sent = '';
			if (!this.loaded) this.#fetchSummary();
		};
		source.addEventListener('hello', (e) => {
			this.#stream = JSON.parse(e.data).id;
			this.#sent = '';
			this.#sync();
		});
		source.addEventListener('summary', (e) => this.#setSummary(JSON.parse(e.data)));
		source.addEventListener('result', (e) => this.#dispatch(JSON.parse(e.data)));
		source.addEventListener('reload', () => this.reloads++);
		source.addEventListener('lagged', () => this.reloads++);
	}

	#dispatch(view: ServiceView) {
		const id = `${view.host}/${view.service}`;
		for (const l of this.#listeners) if (l.services.has(id) || l.hosts.has(view.host)) l.onResult(view);
	}

	/** Sends the union of all watches, once per task. */
	#sync() {
		if (this.#syncing) return;
		this.#syncing = true;
		queueMicrotask(() => {
			this.#syncing = false;
			const stream = this.#stream;
			if (!stream) return;
			const services = new Set<string>();
			const hosts = new Set<string>();
			for (const l of this.#listeners) {
				for (const s of l.services) services.add(s);
				for (const h of l.hosts) hosts.add(h);
			}
			const watch = { services: [...services].sort(), hosts: [...hosts].sort() };
			const key = JSON.stringify(watch);
			if (key === this.#sent) return;
			this.#sent = key;
			api.watch(stream, watch).catch(() => {
				// The stream is gone; its reconnect sends the watch again.
				if (this.#sent === key) this.#sent = '';
			});
		});
	}
}

export const live = new Live();
