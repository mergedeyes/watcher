// Form fields per object type, and the conversion between form values
// and the fields written to the TOML file. Fields the form doesn't know
// are kept as they are.

import type { Kind } from './api.ts';

export type FieldType =
	| 'text'
	| 'mono'
	| 'list'
	| 'bool'
	| 'int'
	| 'duration'
	| 'select'
	| 'vars'
	| 'thresholds'
	| 'selector'
	| 'services';

export interface Field {
	key: string;
	label: string;
	type: FieldType;
	help?: string;
	placeholder?: string;
	options?: string[];
	required?: boolean;
	/** For `bool`: the value when the key is absent. */
	default?: boolean;
	/** Suggestions for `list` fields: names of other objects. */
	suggest?: Kind;
	/** Suggestions for a single name. */
	suggestOne?: Kind;
	/** Suggest the scripts in the check directories. */
	scripts?: boolean;
	/** For `selector`: what it selects. */
	selects?: 'host' | 'service';
}

export interface Group {
	title: string;
	fields: Field[];
}

const NAME: Field = { key: 'name', label: 'Name', type: 'text', required: true };
const TITLE: Field = { key: 'title', label: 'Title', type: 'text', placeholder: 'display name' };
const VARS: Field = { key: 'vars', label: 'Variables', type: 'vars', help: '$name$ in commands' };
const ENABLED: Field = { key: 'enabled', label: 'Enabled', type: 'bool', default: true };

const SCHEDULE: Field[] = [
	{ key: 'interval', label: 'Check Interval', type: 'duration', placeholder: '1m' },
	{ key: 'retry_interval', label: 'Retry Interval', type: 'duration', placeholder: 'min(interval, 1m)' },
	{ key: 'max_attempts', label: 'Max Attempts', type: 'int', placeholder: '3 (passive: 1)' },
	{ key: 'timeout', label: 'Timeout', type: 'duration', placeholder: '60s' }
];

const CHECK: Field[] = [
	{ key: 'command', label: 'Command', type: 'mono', scripts: true, placeholder: 'check_ping -H $host.address$ -w $warn$ -c $crit$' },
	{ key: 'shell', label: 'Shell', type: 'bool', default: false, help: 'run with /bin/sh -c' },
	{ key: 'passive', label: 'Passive', type: 'bool', default: false, help: 'results via API, no command' },
	{ key: 'format', label: 'Format', type: 'select', options: ['auto', 'nagios', 'key_value', 'json'] }
];

const PASSIVE: Field[] = [
	{ key: 'freshness', label: 'Freshness', type: 'duration', placeholder: 'passive only, e.g. 26h' },
	{ key: 'flap_detection', label: 'Flap Detection', type: 'bool', default: true },
	{ key: 'flap_high', label: 'Flap Start %', type: 'int', placeholder: '20' },
	{ key: 'flap_low', label: 'Flap Stop %', type: 'int', placeholder: '5' }
];

const THRESHOLDS: Field = { key: 'thresholds', label: 'Thresholds', type: 'thresholds', help: '> 100, >= 10GB, 10:20, @10:20' };
const GRAPHS: Field = { key: 'graphs', label: 'Graphs', type: 'bool', default: true, help: 'graphs of its metrics' };
const SERVICES: Field = { key: 'services', label: 'Services', type: 'list', suggest: 'service', help: 'comma-separated' };
const SERVICEGROUPS: Field = {
	key: 'servicegroups',
	label: 'Service Groups',
	type: 'list',
	suggest: 'servicegroup',
	help: 'their services, comma-separated'
};
const HOST_CHECK: Field = { key: 'host_check', label: 'Host Check', type: 'text', help: 'service that gives UP/DOWN' };
const GRAPH: Field = { key: 'graph', label: 'Graph', type: 'text', help: 'host page graphs; default: Host Check' };

export const SCHEMAS: Record<Kind, Group[]> = {
	check: [
		{ title: 'Check', fields: [NAME, TITLE, ...CHECK] },
		{ title: 'Evaluation', fields: [THRESHOLDS, { ...VARS, help: 'defaults for $name$ in the command' }, GRAPHS] },
		{ title: 'Scheduling', fields: [...SCHEDULE, ...PASSIVE] }
	],
	service: [
		{
			title: 'Service',
			fields: [
				NAME,
				TITLE,
				{ key: 'check', label: 'Check', type: 'text', suggestOne: 'check', help: 'default: the check named like the service' },
				{ ...CHECK[0]!, help: 'instead of a check' },
				...CHECK.slice(1, 3),
				{ key: 'for_each', label: 'For Each', type: 'text', help: 'host var with a table per item: one service each (disk_root, …)' },
				{ key: 'groups', label: 'Service Groups', type: 'list', suggest: 'servicegroup', help: 'comma-separated' },
				ENABLED
			]
		},
		{ title: 'Evaluation', fields: [THRESHOLDS, { ...VARS, help: 'over the check’s; host vars win' }, GRAPHS] },
		{ title: 'Scheduling', fields: [...SCHEDULE, ...PASSIVE] }
	],
	host: [
		{
			title: 'Host',
			fields: [
				NAME,
				{ key: 'address', label: 'Address', type: 'text', placeholder: 'defaults to name' },
				{ key: 'groups', label: 'Host Groups', type: 'list', suggest: 'hostgroup', help: 'comma-separated' },
				{ key: 'node', label: 'Node', type: 'text', placeholder: 'local' },
				{ ...HOST_CHECK, help: 'service that gives UP/DOWN; default: from host groups' },
				GRAPH
			]
		},
		{ title: 'Services', fields: [SERVICES, SERVICEGROUPS] },
		{
			title: 'Host Services',
			fields: [{ key: 'service', label: 'Overrides', type: 'services', help: 'settings of assigned services on this host, or one-off services' }]
		},
		{ title: 'Other', fields: [VARS, ENABLED] }
	],
	hostgroup: [
		{
			title: 'Host Group',
			fields: [
				NAME,
				TITLE,
				{ key: 'members', label: 'Members', type: 'list', suggest: 'host', help: 'host names, * and ? match; hosts can also list the group' }
			]
		},
		{ title: 'Services', fields: [{ ...SERVICES, help: 'for every member, comma-separated' }, SERVICEGROUPS] },
		{ title: 'Hosts Get', fields: [HOST_CHECK, GRAPH, { ...VARS, help: 'host var defaults' }] }
	],
	servicegroup: [
		{
			title: 'Service Group',
			fields: [NAME, TITLE, { ...SERVICES, help: 'comma-separated; services can also list the group' }]
		}
	],
	contact: [
		{
			title: 'Contact',
			fields: [
				NAME,
				{ key: 'email', label: 'Email', type: 'list', help: 'comma-separated' },
				{ key: 'webhook', label: 'Webhook', type: 'text', placeholder: 'https://…' },
				{ key: 'groups', label: 'Contact Groups', type: 'list', suggest: 'contactgroup', help: 'comma-separated' }
			]
		}
	],
	contactgroup: [
		{
			title: 'Contact Group',
			fields: [NAME, TITLE, { key: 'members', label: 'Members', type: 'list', suggest: 'contact', help: 'contacts, comma-separated' }]
		}
	],
	notify: [
		{
			title: 'Rule',
			fields: [
				{ ...NAME, required: false, help: 'optional' },
				{ key: 'contacts', label: 'Contacts', type: 'list', suggest: 'contact', help: 'comma-separated' },
				{ key: 'contact_groups', label: 'Contact Groups', type: 'list', suggest: 'contactgroup', help: 'comma-separated' }
			]
		},
		{
			title: 'Match',
			fields: [
				{ key: 'hosts', label: 'Hosts', type: 'selector', selects: 'host', help: 'none: all hosts' },
				{ key: 'services', label: 'Services', type: 'selector', selects: 'service', help: 'set: service alerts only' },
				{ key: 'states', label: 'States', type: 'list', help: 'warning, critical, unknown, down; none: all' },
				{ key: 'on', label: 'On', type: 'list', help: 'problem, recovery, flapping; none: all' },
				ENABLED
			]
		}
	],
	sink: []
};

/** Config kinds with their page path and the file new objects go to (as on the server). */
export const KINDS: { kind: Kind; path: string; label: string; one: string; file: string }[] = [
	{ kind: 'check', path: 'checks', label: 'Checks', one: 'Check', file: 'ui/checks.toml' },
	{ kind: 'service', path: 'services', label: 'Services', one: 'Service', file: 'ui/services.toml' },
	{ kind: 'host', path: 'hosts', label: 'Hosts', one: 'Host', file: 'ui/hosts.toml' },
	{ kind: 'hostgroup', path: 'hostgroups', label: 'Host Groups', one: 'Host Group', file: 'ui/groups.toml' },
	{ kind: 'servicegroup', path: 'servicegroups', label: 'Service Groups', one: 'Service Group', file: 'ui/groups.toml' },
	{ kind: 'contact', path: 'contacts', label: 'Contacts', one: 'Contact', file: 'ui/contacts.toml' },
	{ kind: 'contactgroup', path: 'contactgroups', label: 'Contact Groups', one: 'Contact Group', file: 'ui/groups.toml' },
	{ kind: 'notify', path: 'notifications', label: 'Notifications', one: 'Rule', file: 'ui/notify.toml' }
];

/** Selector inputs, by what they select. `hosts.*` is the service selector's host part. */
export const SELECTOR_FIELDS: Record<'host' | 'service', [string, string, string][]> = {
	host: [
		['name', 'Name', 'web*, db01'],
		['address', 'Address', '10.0.1.*'],
		['node', 'Node', 'site-a'],
		['groups', 'Host Groups', 'dc1'],
		['expr', 'Expression', 'host.groups contains "web" or host.node == "dc2"']
	],
	service: [
		['name', 'Name', 'disk_*'],
		['checks', 'Checks', 'disk'],
		['groups', 'Service Groups', 'webshop'],
		['hosts.name', 'Host Name', 'web*'],
		['hosts.groups', 'Host Groups', 'prod'],
		['expr', 'Expression', 'service.name ~ "^disk" and host.groups contains "prod"']
	]
};

/** A host service row: name, check, groups, enabled (`yes`/`no`/``), original name. */
export type ServiceRow = [string, string, string, string, string];

/** Form state: strings for inputs, rows for vars, thresholds and services, a map for selectors. */
export type FormValue =
	| string
	| boolean
	| null
	| [string, string][]
	| [string, string, string][]
	| ServiceRow[]
	| Record<string, string>;
export type FormValues = Record<string, FormValue>;

function fields(kind: Kind): Field[] {
	return SCHEMAS[kind].flatMap((g) => g.fields);
}

function str(v: unknown): string {
	return v === undefined || v === null ? '' : String(v);
}

function list(v: unknown): string[] {
	if (v === undefined || v === null) return [];
	return Array.isArray(v) ? v.map(String) : [String(v)];
}

function items(text: string): string[] {
	return text
		.split(',')
		.map((s) => s.trim())
		.filter(Boolean);
}

/** A deep copy of TOML data; works on Svelte state proxies, unlike structuredClone. */
function copy<T>(v: T): T {
	return JSON.parse(JSON.stringify(v)) as T;
}

function isTable(v: unknown): v is Record<string, unknown> {
	return typeof v === 'object' && v !== null && !Array.isArray(v);
}

/** Form values for a selector: `all` for `{}`, `list` for the shorthand list. */
function selectorToForm(v: unknown, selects: 'host' | 'service'): Record<string, string> {
	const out: Record<string, string> = { all: '', list: '' };
	for (const [key] of SELECTOR_FIELDS[selects]) out[key] = '';
	if (Array.isArray(v)) {
		out.list = list(v).join(', ');
	} else if (isTable(v)) {
		if (Object.keys(v).length === 0) out.all = 'true';
		for (const [key] of SELECTOR_FIELDS[selects]) {
			const [a, b] = key.split('.');
			const value = b ? (isTable(v[a!]) ? (v[a!] as Record<string, unknown>)[b] : undefined) : v[a!];
			out[key] = key === 'expr' ? str(value) : list(value).join(', ');
		}
	}
	return out;
}

/** The selector to write; keys the form doesn't show are kept. */
function selectorFromForm(form: Record<string, string>, raw: unknown, selects: 'host' | 'service'): unknown {
	if (form.list?.trim()) return items(form.list);
	const out: Record<string, unknown> = isTable(raw) ? copy(raw) : {};
	for (const [key] of SELECTOR_FIELDS[selects]) {
		const [a, b] = key.split('.');
		const text = (form[key] ?? '').trim();
		const value = key === 'expr' ? text || undefined : items(text).length ? items(text) : undefined;
		if (b) {
			const sub = isTable(out[a!]) ? (out[a!] as Record<string, unknown>) : {};
			if (value === undefined) delete sub[b];
			else sub[b] = value;
			if (Object.keys(sub).length) out[a!] = sub;
			else delete out[a!];
		} else if (value === undefined) delete out[a!];
		else out[a!] = value;
	}
	if (Object.keys(out).length === 0) return form.all === 'true' ? {} : undefined;
	return out;
}

/** Form values for an object's fields (or for a new object). */
export function toForm(kind: Kind, raw: Record<string, unknown>): FormValues {
	const values: FormValues = {};
	for (const f of fields(kind)) {
		const v = raw[f.key];
		switch (f.type) {
			case 'list':
				values[f.key] = list(v).join(', ');
				break;
			case 'bool':
				values[f.key] = v === undefined ? (f.default ?? false) : v === true;
				break;
			case 'vars':
				values[f.key] = Object.entries(isTable(v) ? v : {})
					.filter(([, x]) => !isTable(x))
					.map(([k, x]) => [k, str(x)] as [string, string]);
				break;
			case 'thresholds':
				values[f.key] = Object.entries((v as Record<string, Record<string, unknown>>) ?? {}).map(
					([metric, t]) => [metric, str(t?.warn), str(t?.crit)] as [string, string, string]
				);
				break;
			case 'selector':
				values[f.key] = selectorToForm(v, f.selects ?? 'host');
				break;
			case 'services':
				values[f.key] = Object.entries(isTable(v) ? v : {}).map(([name, e]) => {
					const entry = isTable(e) ? e : {};
					const enabled = entry.enabled === false ? 'no' : entry.enabled === true ? 'yes' : '';
					return [name, str(entry.check), list(entry.groups).join(', '), enabled, name] as ServiceRow;
				});
				break;
			default:
				values[f.key] = str(v);
		}
	}
	return values;
}

function same(a: unknown, b: unknown) {
	return JSON.stringify(a) === JSON.stringify(b);
}

/** The service entries to write; keys of an entry the form doesn't show are kept. */
function servicesFromForm(rows: ServiceRow[], raw: unknown): unknown {
	const before = isTable(raw) ? raw : {};
	const out: Record<string, unknown> = {};
	for (const [name, check, groups, enabled, orig] of rows) {
		if (!name.trim()) continue;
		const entry: Record<string, unknown> = isTable(before[orig]) ? copy(before[orig]) : {};
		const set = (key: string, value: unknown) => {
			if (value === undefined || value === '') delete entry[key];
			else entry[key] = value;
		};
		set('check', check.trim());
		set('groups', items(groups).length ? items(groups) : undefined);
		set('enabled', enabled === 'no' ? false : enabled === 'yes' ? true : undefined);
		out[name.trim()] = entry;
	}
	return Object.keys(out).length ? out : undefined;
}

/**
 * The fields to write: `raw` with the form's keys replaced. Empty inputs
 * and default booleans remove the key; a value equal to what was written
 * keeps its original form (`groups = "x"` stays a string).
 */
export function fromForm(kind: Kind, raw: Record<string, unknown>, values: FormValues): Record<string, unknown> {
	const out: Record<string, unknown> = { ...raw };
	for (const f of fields(kind)) {
		const v = values[f.key];
		let next: unknown;
		switch (f.type) {
			case 'list': {
				const parts = items(String(v ?? ''));
				next = parts.length === 0 ? undefined : same(parts, list(raw[f.key])) ? raw[f.key] : parts;
				break;
			}
			case 'bool':
				next = v === (f.default ?? false) ? undefined : v;
				// Keep an explicit default that was written.
				if (next === undefined && raw[f.key] === v) next = v;
				break;
			case 'int': {
				const text = String(v ?? '').trim();
				// Not a number: sent as text, so the server reports it.
				next = text === '' ? undefined : Number.isFinite(Number(text)) ? Number(text) : text;
				break;
			}
			case 'vars': {
				const rows = (v as [string, string][]).filter(([k]) => k.trim());
				// Table vars (for_each data) aren't in the form: keep them.
				const tables = Object.entries(isTable(raw[f.key]) ? (raw[f.key] as Record<string, unknown>) : {}).filter(([, x]) =>
					isTable(x)
				);
				const all = [...tables, ...rows.map(([k, x]) => [k.trim(), x] as [string, unknown])];
				next = all.length === 0 ? undefined : Object.fromEntries(all);
				break;
			}
			case 'thresholds': {
				const rows = (v as [string, string, string][]).filter(([m]) => m.trim());
				next =
					rows.length === 0
						? undefined
						: Object.fromEntries(
								rows.map(([m, warn, crit]) => {
									const t: Record<string, string> = {};
									if (warn.trim()) t.warn = warn.trim();
									if (crit.trim()) t.crit = crit.trim();
									return [m.trim(), t];
								})
							);
				break;
			}
			case 'selector':
				next = selectorFromForm(v as Record<string, string>, raw[f.key], f.selects ?? 'host');
				break;
			case 'services':
				next = servicesFromForm(v as ServiceRow[], raw[f.key]);
				break;
			default: {
				const text = String(v ?? '');
				next = text.trim() === '' ? undefined : f.type === 'mono' ? text : text.trim();
			}
		}
		if (next === undefined) delete out[f.key];
		else out[f.key] = same(next, raw[f.key]) ? raw[f.key] : next;
	}
	return out;
}

/** Keys of `raw` the form doesn't show. */
export function otherKeys(kind: Kind, raw: Record<string, unknown>): string[] {
	const known = new Set(fields(kind).map((f) => f.key));
	return Object.keys(raw).filter((k) => !known.has(k));
}

/**
 * Names for clones: comma- or line-separated, with numeric ranges keeping
 * their padding (`web[03-05]` is web03, web04, web05). Throws on errors.
 */
export function expandNames(text: string, max = 1000): string[] {
	const out: string[] = [];
	for (const item of text.split(/[,\n]/).map((s) => s.trim()).filter(Boolean)) {
		const m = item.match(/^(.*)\[(\d+)-(\d+)\](.*)$/);
		if (!m) {
			out.push(item);
			continue;
		}
		const [, before, from, to, after] = m as unknown as [string, string, string, string, string];
		const [a, b] = [Number(from), Number(to)];
		if (b < a) throw new Error(`${item}: range goes down`);
		if (b - a + 1 + out.length > max) throw new Error(`more than ${max} names`);
		for (let n = a; n <= b; n++) out.push(`${before}${String(n).padStart(from.length, '0')}${after}`);
	}
	const dup = out.find((n, i) => out.indexOf(n) !== i);
	if (dup) throw new Error(`${dup} is listed twice`);
	if (out.length > max) throw new Error(`more than ${max} names`);
	return out;
}
