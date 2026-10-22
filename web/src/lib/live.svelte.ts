// Live state of all services: loaded once, then patched from the hub's
// event stream. After a reconnect or a config reload the lists are fetched
// again, so nothing missed while disconnected stays stale.

import { api, type HostView, type ServiceView } from './api.ts';

export type Connection = 'connecting' | 'live' | 'offline';

class Live {
	services = $state<ServiceView[]>([]);
	hosts = $state<HostView[]>([]);
	connection = $state<Connection>('connecting');
	loaded = $state(false);
	error = $state<string | null>(null);
	/** Ticks every 10 s, for ages like "12m". */
	now = $state(Date.now());

	#index = new Map<string, number>();
	#source: EventSource | null = null;

	start() {
		if (this.#source) return;
		setInterval(() => (this.now = Date.now()), 10_000);
		this.#connect();
	}

	find(host: string, service: string): ServiceView | undefined {
		const i = this.#index.get(`${host}/${service}`);
		return i === undefined ? undefined : this.services[i];
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
		source.addEventListener('reload', () => this.reload());
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

	/** Host counts and worst state, from the live service list. */
	#recount(host: HostView) {
		const counts = { ok: 0, warning: 0, critical: 0, unknown: 0, pending: 0 };
		let worst: HostView['state'] = null;
		const rank = { ok: 0, warning: 1, unknown: 2, critical: 3 } as const;
		for (const s of this.services) {
			if (s.host !== host.name) continue;
			if (s.state === null) {
				counts.pending++;
				continue;
			}
			counts[s.state]++;
			if (worst === null || rank[s.state] > rank[worst]) worst = s.state;
		}
		host.services = counts;
		host.state = worst;
	}
}

export const live = new Live();
