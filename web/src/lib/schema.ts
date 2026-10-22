// Form fields per object type, and the conversion between form values
// and the fields written to the TOML file. Fields the form doesn't know
// are kept as they are.

import type { Kind } from './api.ts';

export type FieldType =
	| 'text'
	| 'mono'
	| 'list'
	| 'bool'
	| 'tristate'
	| 'int'
	| 'duration'
	| 'select'
	| 'vars'
	| 'thresholds';

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
}

export interface Group {
	title: string;
	fields: Field[];
}

const NAME: Field = { key: 'name', label: 'Name', type: 'text', required: true };
const USE: Field = { key: 'use', label: 'Templates', type: 'list', suggest: 'template', help: 'comma-separated' };
const VARS: Field = { key: 'vars', label: 'Variables', type: 'vars', help: '$host.vars.x$ / $check.vars.x$' };

const HOST: Field[] = [
	{ key: 'address', label: 'Address', type: 'text', placeholder: 'defaults to name' },
	{ key: 'check', label: 'Host Check', type: 'text', suggestOne: 'check', help: 'gives UP/DOWN; none: always UP' },
	{ key: 'graph', label: 'Graph', type: 'text', suggestOne: 'check', help: 'host page graphs; default: Host Check' },
	{ key: 'tags', label: 'Tags', type: 'list', help: 'comma-separated' },
	{ key: 'node', label: 'Node', type: 'text', placeholder: 'local' }
];

const CHECK: Field[] = [
	{ key: 'apply_to', label: 'Apply To', type: 'mono', placeholder: '"linux" in host.tags' },
	{ key: 'hosts', label: 'Hosts', type: 'list', suggest: 'host', help: 'comma-separated, in addition to Apply To' },
	{ key: 'command', label: 'Command', type: 'mono', scripts: true, placeholder: 'check_ping -H $host.address$ -w 100,20% -c 500,60%' },
	{ key: 'shell', label: 'Shell', type: 'bool', default: false, help: 'run with /bin/sh -c' },
	{ key: 'passive', label: 'Passive', type: 'bool', default: false, help: 'results via API, no command' },
	{ key: 'format', label: 'Format', type: 'select', options: ['auto', 'nagios', 'key_value', 'json'] }
];

const SCHEDULE: Field[] = [
	{ key: 'interval', label: 'Check Interval', type: 'duration', placeholder: '1m' },
	{ key: 'retry_interval', label: 'Retry Interval', type: 'duration', placeholder: 'min(interval, 1m)' },
	{ key: 'max_attempts', label: 'Max Attempts', type: 'int', placeholder: '3 (passive: 1)' },
	{ key: 'timeout', label: 'Timeout', type: 'duration', placeholder: '60s' },
	{ key: 'freshness', label: 'Freshness', type: 'duration', placeholder: 'passive only, e.g. 26h' },
	{ key: 'flap_detection', label: 'Flap Detection', type: 'bool', default: true },
	{ key: 'flap_high', label: 'Flap Start %', type: 'int', placeholder: '20' },
	{ key: 'flap_low', label: 'Flap Stop %', type: 'int', placeholder: '5' }
];

const ALERTING: Field[] = [
	{ key: 'notify', label: 'Notify', type: 'list', suggest: 'contact', help: 'contacts, comma-separated' },
	{ key: 'thresholds', label: 'Thresholds', type: 'thresholds', help: '> 100, >= 10GB, 10:20, @10:20' }
];

const ENABLED: Field = { key: 'enabled', label: 'Enabled', type: 'bool', default: true };

/** Templates set any field; an unset one is left to the object. */
function optional(fields: Field[]): Field[] {
	return fields.map((f) => (f.type === 'bool' ? { ...f, type: 'tristate' } : f));
}

export const SCHEMAS: Record<Kind, Group[]> = {
	host: [{ title: 'Host', fields: [NAME, ...HOST, USE, VARS, ENABLED] }],
	check: [
		{ title: 'Check', fields: [NAME, USE, ...CHECK] },
		{ title: 'Scheduling', fields: SCHEDULE },
		{ title: 'Alerting', fields: ALERTING },
		{ title: 'Other', fields: [VARS, ENABLED] }
	],
	template: [
		{ title: 'Template', fields: [{ ...NAME, help: 'used as: use = "name"' }, USE] },
		{ title: 'Check', fields: optional(CHECK) },
		{ title: 'Scheduling', fields: SCHEDULE },
		{ title: 'Alerting', fields: ALERTING },
		{ title: 'Host', fields: HOST },
		{ title: 'Other', fields: optional([VARS, ENABLED]) }
	],
	contact: [
		{
			title: 'Contact',
			fields: [
				NAME,
				{ key: 'email', label: 'Email', type: 'list', help: 'comma-separated' },
				{ key: 'webhook', label: 'Webhook', type: 'text', placeholder: 'https://…' }
			]
		}
	],
	sink: []
};

export const KINDS: { kind: Kind; path: string; label: string; one: string }[] = [
	{ kind: 'host', path: 'hosts', label: 'Hosts', one: 'Host' },
	{ kind: 'check', path: 'checks', label: 'Checks', one: 'Check' },
	{ kind: 'contact', path: 'contacts', label: 'Contacts', one: 'Contact' },
	{ kind: 'template', path: 'templates', label: 'Templates', one: 'Template' }
];

/** Form state: strings for inputs, rows for vars and thresholds. */
export type FormValue = string | boolean | null | [string, string][] | [string, string, string][];
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
			case 'tristate':
				values[f.key] = v === undefined ? null : v === true;
				break;
			case 'vars':
				values[f.key] = Object.entries((v as Record<string, unknown>) ?? {}).map(
					([k, x]) => [k, str(x)] as [string, string]
				);
				break;
			case 'thresholds':
				values[f.key] = Object.entries((v as Record<string, Record<string, unknown>>) ?? {}).map(
					([metric, t]) => [metric, str(t?.warn), str(t?.crit)] as [string, string, string]
				);
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

/**
 * The fields to write: `raw` with the form's keys replaced. Empty inputs
 * and default booleans remove the key; a value equal to what was written
 * keeps its original form (`use = "x"` stays a string).
 */
export function fromForm(kind: Kind, raw: Record<string, unknown>, values: FormValues): Record<string, unknown> {
	const out: Record<string, unknown> = { ...raw };
	for (const f of fields(kind)) {
		const v = values[f.key];
		let next: unknown;
		switch (f.type) {
			case 'list': {
				const items = String(v ?? '')
					.split(',')
					.map((s) => s.trim())
					.filter(Boolean);
				next = items.length === 0 ? undefined : same(items, list(raw[f.key])) ? raw[f.key] : items;
				break;
			}
			case 'bool':
				next = v === (f.default ?? false) ? undefined : v;
				// Keep an explicit default that was written.
				if (next === undefined && raw[f.key] === v) next = v;
				break;
			case 'tristate':
				next = v === null ? undefined : v;
				break;
			case 'int': {
				const text = String(v ?? '').trim();
				// Not a number: sent as text, so the server reports it.
				next = text === '' ? undefined : Number.isFinite(Number(text)) ? Number(text) : text;
				break;
			}
			case 'vars': {
				const rows = (v as [string, string][]).filter(([k]) => k.trim());
				next = rows.length === 0 ? undefined : Object.fromEntries(rows.map(([k, x]) => [k.trim(), x]));
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
