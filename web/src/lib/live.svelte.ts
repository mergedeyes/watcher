// Live state of all services: loaded once, then patched from the hub's
// event stream. After a reconnect or a config reload the lists are fetched
// again, so nothing missed while disconnected stays stale.
//
// Paused, the stream is closed and the lists only change on refresh(). Pages
// refetch their own data when the lists change, so they hold still too.

import { api, type HostView, type ServiceView } from './api.ts';

export type Connection = 'connecting' | 'live' | 'offline' | 'paused';

class Live {
	services = $state<ServiceView[]>([]);
	hosts = $state<HostView[]>([]);
	connection = $state<Connection>('connecting');
	loaded = $state(false);
	error = $state<string | null>(null);
	/** Ticks every 10 s, for ages like "12m". */
	now = $state(Date.now());
	/** Counts config reloads on the server. */
	reloads = $state(0);

	#index = new Map<string, number>();
	#source: EventSource | null = null;
	#timer: ReturnType<typeof setInterval> | undefined;
	#started = false;

	start() {
		if (this.#started) return;
		this.#started = true;
		this.#timer = setInterval(() => (this.now = Date.now()), 10_000);
		if (this.connection === 'paused') this.reload();
		else this.#connect();
	}

	/** Stops updates until resume(); refresh() fetches once. */
	pause() {
		this.#source?.close();
		this.#source = null;
		this.connection = 'paused';
	}

	resume() {
		if (this.connection !== 'paused') return;
		this.connection = 'connecting';
		if (this.#started) this.#connect();
	}

	async refresh() {
		this.now = Date.now();
		await this.reload();
	}

	/** Disconnects and forgets everything, e.g. after logging out. */
	stop() {
		this.#source?.close();
		this.#source = null;
		this.#started = false;
		clearInterval(this.#timer);
		this.services = [];
		this.hosts = [];
		this.#index.clear();
		this.loaded = false;
		this.error = null;
		this.connection = 'connecting';
	}

	/** A service's title, else its name. */
	title(host: string, service: string): string {
		return this.find(host, service)?.title ?? service;
	}

	find(host: string, service: string): ServiceView | undefined {
		// Read the reactive list first: callers must depend on it even while
		// it is still empty, or they never see it load.
		const services = this.services;
		const i = this.#index.get(`${host}/${service}`);
		return i === undefined ? undefined : services[i];
	}

	async reload() {
		try {
			const [services, hosts] = await Promise.all([api.services(), api.hosts()]);
			this.services = services.services;
			this.hosts = hosts;
			this.#index = new Map(this.services.map((s, i) => [`${s.host}/${s.service}`, i]));
			this.error = null;
			this.loaded = true;
		} catch (e) {
			this.error = e instanceof Error ? e.message : String(e);
		}
	}

	#connect() {
		const source = new EventSource('/api/v1/stream');
		this.#source = source;
		source.onopen = () => {
			this.connection = 'live';
			this.reload();
		};
		// EventSource reconnects by itself; onopen then reloads.
		source.onerror = () => {
			this.connection = 'offline';
			if (!this.loaded) this.reload();
		};
		source.addEventListener('result', (e) => this.#patch(JSON.parse(e.data)));
		source.addEventListener('reload', () => {
			this.reloads++;
			this.reload();
		});
		source.addEventListener('lagged', () => this.reload());
	}

	#patch(view: ServiceView) {
		const i = this.#index.get(`${view.host}/${view.service}`);
		if (i === undefined) {
			this.reload();
			return;
		}
		this.services[i] = view;
		const host = this.hosts.find((h) => h.name === view.host);
		if (host) this.#recount(host);
	}

	/** Host counts, worst service state and host state, from the live service list. */
	#recount(host: HostView) {
		const counts = { ok: 0, warning: 0, critical: 0, unknown: 0, pending: 0 };
		let worst: HostView['worst'] = null;
		const rank = { ok: 0, warning: 1, unknown: 2, critical: 3 } as const;
		for (const s of this.services) {
			if (s.host !== host.name) continue;
			if (s.service === host.check) this.#hostState(host, s);
			if (s.state === null) {
				counts.pending++;
				continue;
			}
			counts[s.state]++;
			if (worst === null || rank[s.state] > rank[worst]) worst = s.state;
		}
		host.services = counts;
		host.worst = worst;
	}

	/** The host state from its host check: OK/WARNING is UP (confirmed), else DOWN. */
	#hostState(host: HostView, check: ServiceView) {
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
}

export const live = new Live();
