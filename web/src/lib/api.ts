// Types and calls for the hub API (/api/v1). Mirrors crates/hub/src/view.rs.

export type State = 'ok' | 'warning' | 'critical' | 'unknown';
export type StateType = 'soft' | 'hard';

export interface ServiceView {
	host: string;
	service: string;
	/** null while pending (no result yet) */
	state: State | null;
	state_type: StateType | null;
	attempt: number | null;
	max_attempts: number;
	last_check: string | null;
	last_change: string | null;
	output: string;
	passive: boolean;
}

export interface Counts {
	ok: number;
	warning: number;
	critical: number;
	unknown: number;
	pending: number;
}

export interface HostView {
	name: string;
	address: string;
	node: string | null;
	tags: string[];
	vars: Record<string, string>;
	enabled: boolean;
	state: State | null;
	services: Counts;
}

export interface Metric {
	name: string;
	value: number;
	unit?: string;
	labels?: Record<string, string>;
	warn?: string;
	crit?: string;
	min?: number;
	max?: number;
	/** absent when the metric has no thresholds */
	state?: State;
}

export interface ServiceDetail extends ServiceView {
	last_hard_change: string | null;
	long_output: string;
	duration_ms: number | null;
	metrics: Metric[];
	check: {
		command: string | null;
		interval_s: number;
		retry_interval_s: number;
		timeout_s: number;
		freshness_s: number | null;
		origin: string;
	};
}

export interface Series {
	id: number;
	metric: string;
	labels: Record<string, string>;
	unit: string;
}

export interface Points {
	bucket: number;
	/** [unix ms, avg, min, max, count] */
	points: [number, number, number, number, number][];
}

export interface StateEvent {
	host: string;
	service: string;
	ts: string;
	from_state: State | null;
	to_state: State;
	state_type: StateType;
	hard: boolean;
	output: string;
}

export class ApiError extends Error {
	constructor(
		public status: number,
		message: string
	) {
		super(message);
	}
	/** The hub runs without a database, so there is no history. */
	get noDatabase() {
		return this.status === 503 && this.message.includes('no database');
	}
}

async function get<T>(path: string, init?: RequestInit): Promise<T> {
	const res = await fetch(`/api/v1${path}`, init);
	if (!res.ok) {
		let message = `${res.status} ${res.statusText}`;
		try {
			message = (await res.json()).error ?? message;
		} catch {
			// not JSON
		}
		throw new ApiError(res.status, message);
	}
	return res.json();
}

const seg = encodeURIComponent;

export const api = {
	services: () => get<{ counts: Counts; services: ServiceView[] }>('/services'),
	hosts: () => get<HostView[]>('/hosts'),
	host: (name: string) =>
		get<{ host: HostView; services: ServiceView[]; origin: string }>(`/hosts/${seg(name)}`),
	service: (host: string, service: string) =>
		get<ServiceDetail>(`/services/${seg(host)}/${seg(service)}`),
	series: (host: string, service: string) =>
		get<Series[]>(`/services/${seg(host)}/${seg(service)}/series`),
	points: (id: number, from: string, points: number, signal?: AbortSignal) =>
		get<Points>(`/series/${id}/points?from=${seg(from)}&points=${points}`, { signal }),
	events: (host: string, service: string, limit = 20) =>
		get<StateEvent[]>(`/events?host=${seg(host)}&service=${seg(service)}&limit=${limit}`)
};

export function serviceHref(host: string, service: string) {
	return `/services/${seg(host)}/${seg(service)}`;
}

export function hostHref(host: string) {
	return `/hosts/${seg(host)}`;
}
