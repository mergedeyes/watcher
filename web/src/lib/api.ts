// Types and calls for the hub API (/api/v1). Mirrors crates/hub/src/view.rs.

export type State = 'ok' | 'warning' | 'critical' | 'unknown';
export type HostState = 'up' | 'down';
export type StateType = 'soft' | 'hard';

export interface ServiceView {
	host: string;
	service: string;
	/** For display; null shows the name. */
	title: string | null;
	/** null while pending (no result yet) */
	state: State | null;
	state_type: StateType | null;
	attempt: number | null;
	max_attempts: number;
	last_check: string | null;
	last_change: string | null;
	output: string;
	passive: boolean;
	/** Show graphs of its metrics. */
	graphs: boolean;
	/** The state changes too often; problem notifications are held back. */
	flapping: boolean;
	/** Service groups. */
	groups: string[];
	acknowledged: boolean;
	/** Notifications off for it or its host. */
	muted: boolean;
	checks_enabled: boolean;
	/** In a downtime now. */
	downtime: boolean;
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
	/** For run_on = "host" services. */
	ssh: { user: string; port: number; secret: string } | null;
	/** Text, or tables (for_each data). */
	vars: Record<string, unknown>;
	/** Host groups. */
	groups: string[];
	enabled: boolean;
	/** From the host check; 'up' without one; null while the host check is pending. */
	state: HostState | null;
	state_type: StateType | null;
	attempt: number | null;
	max_attempts: number | null;
	last_check: string | null;
	last_change: string | null;
	output: string;
	flapping: boolean;
	/** Name of the host check. */
	check: string | null;
	/** The service whose graphs the host page shows. */
	graph: string | null;
	/** Worst service state; null if all are pending. */
	worst: State | null;
	services: Counts;
	/** The host problem is acknowledged. */
	acknowledged: boolean;
	muted: boolean;
	checks_enabled: boolean;
	downtime: boolean;
}

/** Acknowledgement, mute or disabled checks on a host or service. */
export interface Flag {
	kind: 'ack' | 'mute' | 'no_checks';
	/** `host` or `host/service`. */
	target: string;
	author: string;
	comment: string;
	created: string;
}

export interface Downtime {
	/** null: a window of a downtime group's schedule. */
	id: number | null;
	kind: 'host' | 'service' | 'hostgroup' | 'servicegroup' | 'downtimegroup';
	target: string;
	start: string;
	end: string;
	author: string | null;
	comment: string;
	/** Schedule entry, e.g. `sun 02:00-04:00`. */
	window: string | null;
	active: boolean;
	hosts: number;
	services: number;
}

export type Action = 'recheck' | 'ack' | 'unack' | 'mute' | 'unmute' | 'disable-checks' | 'enable-checks' | 'downtime';

/** What an action applies to, and its options. */
export interface ActionBody {
	hosts?: string[];
	/** `host/service` ids. */
	services?: string[];
	hostgroups?: string[];
	servicegroups?: string[];
	downtimegroups?: string[];
	comment?: string;
	start?: string;
	end?: string;
	duration?: string;
}

export interface ActionResult {
	action: Action;
	done: string[];
	skipped: { target: string; reason: string }[];
}

export interface ProcessInfo {
	version: string;
	pid: number;
	instance: string | null;
	started: string;
	uptime_s: number;
	config_loaded: string;
	database: string | null;
	notifications: boolean;
	checks: boolean;
	hosts: number;
	services: number;
	scheduler: { jobs: number; running: number; paused: number; passive: number };
	results: { last_1m: number; last_5m: number; last_15m: number };
	acknowledged: number;
	muted: number;
	checks_off: number;
	in_downtime: number;
}

export interface Scheduled {
	host: string;
	service: string;
	next_check: string | null;
	last_check: string | null;
	running: boolean;
	passive: boolean;
	paused: boolean;
	interval_s: number;
}

export interface Metric {
	name: string;
	value: number;
	unit?: string;
	labels?: Record<string, string>;
	/** evaluated thresholds */
	warn?: string;
	crit?: string;
	/** thresholds from the plugin output; Nagios ones are not evaluated */
	perf_warn?: string;
	perf_crit?: string;
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
	/** Weighted state changes in the last 21 results, 0-100. */
	percent_change: number | null;
	/** Gives the host its UP/DOWN state. */
	host_check: boolean;
	check: {
		/** The check it runs; null for an inline command. */
		name: string | null;
		/** The service object it is made from (`disk` for the for_each service `disk_root`); null for a host's one-off service. */
		service: string | null;
		/** Why the host has it: `hostgroup linux`, `servicegroup basics on host web01`, ... */
		assigned: string[];
		command: string | null;
		interval_s: number;
		retry_interval_s: number;
		timeout_s: number;
		freshness_s: number | null;
		/** Flap detection thresholds in percent; null when off. */
		flap_low: number | null;
		flap_high: number | null;
		origin: string;
	};
	/** Downtimes it is in, e.g. `hostgroup linux`. */
	downtimes: string[];
	/** Set on it or its host. */
	flags: Flag[];
}

export interface Series {
	id: number;
	metric: string;
	labels: Record<string, string>;
	unit: string;
	/** Oldest and newest sample; null without samples. */
	first: string | null;
	last: string | null;
}

export interface Points {
	bucket: number;
	/** [unix ms, avg, min, max, count] */
	points: [number, number, number, number, number][];
}

/** How long after a result its history is in the database (ms): the hub writes in batches of about a second. */
export const HISTORY_DELAY = 1500;

/** `host`: UP/DOWN of the host, from its host check. */
export type EventKind = 'service' | 'host' | 'flap_start' | 'flap_stop' | 'ack' | 'downtime_start' | 'downtime_end';

/** An entry of the alert history. */
export interface StateEvent {
	host: string;
	service: string;
	ts: string;
	kind: EventKind;
	/** up/down for host events */
	from_state: State | HostState | null;
	to_state: State | HostState;
	state_type: StateType;
	hard: boolean;
	output: string;
}

export interface EventQuery {
	host?: string;
	service?: string;
	/** comma-separated: service, host, flapping */
	type?: string;
	/** comma-separated states changed to */
	state?: string;
	state_type?: StateType;
	/** RFC 3339, unix seconds or a duration back from now (`24h`) */
	since?: string;
	before?: string;
	limit?: number;
}

/** A secret for checks over SSH, without its value. */
export interface Secret {
	name: string;
	kind: 'key' | 'password';
	/** Keys: the line for authorized_keys. */
	public_key: string | null;
	fingerprint: string | null;
	/** Hosts that log in with it. */
	hosts: string[];
	created: string;
	updated: string;
}

export interface NewSecret {
	name: string;
	kind: 'generate' | 'key' | 'password';
	private_key?: string;
	passphrase?: string;
	password?: string;
	replace?: boolean;
}

/** A pinned SSH host key, and a changed one waiting for acceptance. */
export interface HostKey {
	address: string;
	port: number;
	key: string;
	fingerprint: string | null;
	pinned: string;
	offered: string | null;
	offered_fingerprint: string | null;
	offered_at: string | null;
	hosts: string[];
}

export interface User {
	id: number;
	username: string;
	admin: boolean;
	disabled: boolean;
	created: string;
	last_login: string | null;
}

export interface AuthStatus {
	/** Whether this server requires a login at all. */
	required: boolean;
	user: User | null;
	/** False until the first account is created on the server. */
	users_exist: boolean;
	/** The server runs the first-run setup wizard. */
	setup?: boolean;
}

export interface ApiToken {
	id: number;
	name: string;
	created: string;
	last_used: string | null;
}

export type Kind =
	| 'check'
	| 'service'
	| 'host'
	| 'hostgroup'
	| 'servicegroup'
	| 'contact'
	| 'contactgroup'
	| 'downtimegroup'
	| 'notify'
	| 'sink';

/** Kinds with a Config Tree. A `service` named `host/service` is a service on a host, otherwise a service object. */
export type TreeKind = Exclude<Kind, 'sink'>;

/** A node of a Config Tree: where an object and its values come from. */
export interface TreeNode {
	/** An object kind, or `section` / `value`. */
	kind: TreeKind | 'section' | 'value';
	/** Object name to link to (`host/service` for services on hosts). */
	name?: string;
	label: string;
	/** A value, or why something applies. */
	detail?: string;
	/** Relative to the object directory. */
	file?: string;
	line?: number;
	children?: TreeNode[];
}

/** A config error or warning; `path` is relative to the object directory. */
export interface Problem {
	path: string | null;
	line: number;
	severity: 'error' | 'warning';
	message: string;
}

export interface ConfigOverview {
	mode: 'hybrid' | 'files' | 'ui';
	writable: boolean;
	dir: string;
	files: { path: string; hash: string; size: number }[];
	/** Problems of the files as they are on disk. */
	diagnostics: Problem[];
}

export interface ConfigFile {
	path: string;
	hash: string;
	text: string;
}

/** An object as written in its file, before templates apply. */
export interface ConfigObject {
	kind: Kind;
	name: string;
	path: string;
	line: number;
	fields: Record<string, unknown>;
}

/** A file change: `base` is the hash it was made on (null: new file), `text` null deletes. */
export interface Change {
	path: string;
	base: string | null;
	text: string | null;
}

export interface Delta {
	added: string[];
	removed: string[];
	changed: string[];
}

export interface Changes {
	hosts: Delta;
	services: Delta;
	/** As `hostgroup x`, `servicegroup x`, `contactgroup x`. */
	groups: Delta;
	contacts: Delta;
	/** Notify rules by name or place. */
	notify: Delta;
	sinks: Delta;
}

export type Preview =
	| { valid: true; changes: Changes; diagnostics: Problem[] }
	| { valid: false; diagnostics: Problem[] };

/** Body of a 409 from preview/apply: the file changed since it was read. */
export interface Conflict {
	error: string;
	path: string;
	current: ConfigFile | null;
}

export class ApiError extends Error {
	constructor(
		public status: number,
		message: string,
		/** The parsed JSON error body, for errors that carry details. */
		public body: Record<string, unknown> = {}
	) {
		super(message);
	}
	/** The hub runs without a database, so there is no history. */
	get noDatabase() {
		return this.status === 503 && this.message.includes('no database');
	}
}

/** Called on any 401: the session is missing or expired. */
let onUnauthorized: () => void = () => {};
export function setUnauthorizedHandler(handler: () => void) {
	onUnauthorized = handler;
}

async function get<T>(path: string, init?: RequestInit): Promise<T> {
	const res = await fetch(`/api/v1${path}`, init);
	if (!res.ok) {
		if (res.status === 401 && !path.startsWith('/auth/')) onUnauthorized();
		let message = `${res.status} ${res.statusText}`;
		let body = {};
		try {
			body = await res.json();
			message = (body as { error?: string }).error ?? message;
		} catch {
			// not JSON
		}
		throw new ApiError(res.status, message, body);
	}
	if (res.status === 204) return undefined as T;
	return res.json();
}

function send<T>(method: string, path: string, body?: unknown): Promise<T> {
	return get<T>(path, {
		method,
		headers: body === undefined ? {} : { 'content-type': 'application/json' },
		body: body === undefined ? undefined : JSON.stringify(body)
	});
}

const seg = encodeURIComponent;

export const api = {
	health: () => get<{ status: string; version?: string; instance?: string | null }>('/health'),
	services: () => get<{ counts: Counts; services: ServiceView[] }>('/services'),
	hosts: () => get<HostView[]>('/hosts'),
	host: (name: string) =>
		get<{ host: HostView; services: ServiceView[]; origin: string; flags: Flag[] }>(`/hosts/${seg(name)}`),
	service: (host: string, service: string) =>
		get<ServiceDetail>(`/services/${seg(host)}/${seg(service)}`),
	series: (host: string, service: string) =>
		get<Series[]>(`/services/${seg(host)}/${seg(service)}/series`),
	points: (id: number, from: string, points: number, signal?: AbortSignal) =>
		get<Points>(`/series/${id}/points?from=${seg(from)}&points=${points}`, { signal }),
	events: (q: EventQuery) => {
		const params = new URLSearchParams();
		for (const [k, v] of Object.entries(q)) if (v !== undefined && v !== '') params.set(k, String(v));
		return get<StateEvent[]>(`/events?${params}`);
	},

	action: (action: Action, body: ActionBody) => send<ActionResult>('POST', `/actions/${action}`, body),
	/** A passive result, e.g. from the Submit Result dialog. */
	submit: (host: string, service: string, state: State, output: string) =>
		send<ServiceDetail>('POST', `/services/${seg(host)}/${seg(service)}/result`, { state, output }),
	flags: () => get<Flag[]>('/flags'),
	downtimes: () => get<Downtime[]>('/downtimes'),
	cancelDowntime: (id: number) => send<void>('DELETE', `/downtimes/${id}`),
	process: () => get<ProcessInfo>('/process'),
	setProcess: (switches: { notifications?: boolean; checks?: boolean }) =>
		send<{ notifications: boolean; checks: boolean }>('PUT', '/process', switches),
	scheduler: (limit = 500) => get<Scheduled[]>(`/scheduler?limit=${limit}`),

	authStatus: () => get<AuthStatus>('/auth/status'),
	login: (username: string, password: string) =>
		send<{ user: User }>('POST', '/auth/login', { username, password }),
	logout: () => send<void>('POST', '/auth/logout'),
	tokens: () => get<ApiToken[]>('/tokens'),
	createToken: (name: string) => send<{ token: string; info: ApiToken }>('POST', '/tokens', { name }),
	deleteToken: (id: number) => send<void>('DELETE', `/tokens/${id}`),
	users: () => get<User[]>('/users'),
	createUser: (username: string, password: string, admin: boolean) =>
		send<User>('POST', '/users', { username, password, admin }),
	/** Admins: anything; everyone: their own password. */
	updateUser: (name: string, change: { password?: string; admin?: boolean; disabled?: boolean }) =>
		send<User>('PUT', `/users/${seg(name)}`, change),
	deleteUser: (name: string) => send<void>('DELETE', `/users/${seg(name)}`),

	secrets: () => get<Secret[]>('/ssh/secrets'),
	createSecret: (secret: NewSecret) => send<Secret>('POST', '/ssh/secrets', secret),
	deleteSecret: (name: string) => send<void>('DELETE', `/ssh/secrets/${seg(name)}`),
	hostKeys: () => get<HostKey[]>('/ssh/hostkeys'),
	acceptHostKey: (address: string, port: number) =>
		send<HostKey>('POST', `/ssh/hostkeys/${seg(address)}/${port}/accept`),
	forgetHostKey: (address: string, port: number) => send<void>('DELETE', `/ssh/hostkeys/${seg(address)}/${port}`),

	config: () => get<ConfigOverview>('/config'),
	configFile: (path: string) => get<ConfigFile>(`/config/files/${path.split('/').map(seg).join('/')}`),
	configObjects: () => get<{ objects: ConfigObject[]; diagnostics: Problem[] }>('/config/objects'),
	groups: () =>
		get<{
			hostgroups: { name: string; title: string | null }[];
			servicegroups: { name: string; title: string | null }[];
			downtimegroups: { name: string; title: string | null; schedule: string[] }[];
		}>('/groups'),
	configTree: (kind: TreeKind, name: string) =>
		get<TreeNode>(`/config/tree/${kind}/${name.split('/').map(seg).join('/')}`),
	/** Executables in the check directories, where bare command names are looked up. */
	configScripts: () => get<{ dirs: string[]; scripts: { name: string; dir: string }[] }>('/config/scripts'),
	/** The file change for creating (no name), updating or deleting (no fields) an object. */
	editObject: (kind: Kind, name: string | null, fields: Record<string, unknown> | null) =>
		send<Change>('POST', '/config/edit', { kind, name, fields }),
	/** File changes for several objects at once (clones); `path` puts new objects into that file. */
	editMany: (
		edits: { kind: Kind; name: string | null; fields: Record<string, unknown> | null; path?: string }[]
	) => send<{ changes: Change[] }>('POST', '/config/edit-many', { edits }),
	/** What selectors match in the running config, with why. */
	match: (kind: 'host' | 'service', selectors: unknown[]) =>
		send<{ total: number; matches: { name: string; why: string }[] }>('POST', '/config/match', { kind, selectors }),
	preview: (changes: Change[]) => send<Preview>('POST', '/config/preview', { changes }),
	apply: (changes: Change[]) =>
		send<{ changes: Changes; diagnostics: Problem[] }>('POST', '/config/apply', { changes })
};

export function serviceHref(host: string, service: string) {
	return `/services/${seg(host)}/${seg(service)}`;
}

export function hostHref(host: string) {
	return `/hosts/${seg(host)}`;
}
