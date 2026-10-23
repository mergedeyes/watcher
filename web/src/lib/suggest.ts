// Suggestions for config form fields: names of objects a field refers to,
// and values already used for the same field elsewhere in the config.

import type { ConfigObject, Kind } from './api.ts';
import type { Field } from './schema.ts';

/** Built-in checks (crates/exec/src/builtin.rs). */
const BUILTIN_CHECKS = ['ping'];

export interface Option {
	value: string;
	/** Shown muted next to the value, e.g. where it comes from. */
	hint?: string;
}

const DURATIONS = ['10s', '30s', '1m', '2m', '5m', '10m', '15m', '30m', '1h', '6h', '12h', '26h'];
const STATES = ['warning', 'critical', 'unknown', 'down'];
const ON = ['problem', 'recovery', 'flapping'];

function isTable(v: unknown): v is Record<string, unknown> {
	return typeof v === 'object' && v !== null && !Array.isArray(v);
}

/** Scalars of `v`: the value itself, or the items of a list. */
function scalars(v: unknown): string[] {
	if (v === undefined || v === null || isTable(v)) return [];
	return (Array.isArray(v) ? v : [v]).filter((x) => !isTable(x) && !Array.isArray(x)).map(String);
}

function uniq(values: string[]): string[] {
	return [...new Set(values.filter((v) => v.trim() !== ''))].sort((a, b) => a.localeCompare(b, undefined, { numeric: true }));
}

const UNIT_SECS: Record<string, number> = { ms: 0.001, s: 1, m: 60, h: 3600, d: 86400, w: 604800 };

/** Seconds in a duration like `90s`, `1h30m`; NaN if it isn't one. */
export function durationSecs(text: string): number {
	const parts = [...text.trim().matchAll(/(\d+(?:\.\d+)?)\s*(ms|s|m|h|d|w)/g)];
	if (parts.length === 0 || parts.map((p) => p[0]).join('') !== text.replace(/\s+/g, '')) return NaN;
	return parts.reduce((sum, [, n, unit]) => sum + Number(n) * UNIT_SECS[unit!]!, 0);
}

/** Durations, shortest first; other text after them. */
function byDuration(values: string[]): string[] {
	const secs = (v: string) => {
		const n = durationSecs(v);
		return Number.isNaN(n) ? Infinity : n;
	};
	return [...values].sort((a, b) => secs(a) - secs(b) || a.localeCompare(b));
}

/** Suggestions for one config form, built from all objects. */
export class Suggest {
	constructor(
		private objects: ConfigObject[],
		/** Names of the services that exist (from the live view). */
		private services: string[],
		private scripts: { name: string; dir: string }[]
	) {}

	private of(kind: Kind) {
		return this.objects.filter((o) => o.kind === kind);
	}

	names(kind: Kind): Option[] {
		return this.of(kind).map((o) => {
			const title = o.fields.title;
			return { value: o.name, hint: typeof title === 'string' ? title : undefined };
		});
	}

	/** Checks, and the built-in ones no check of the same name replaces. */
	checkNames(): Option[] {
		const out = this.names('check');
		for (const value of BUILTIN_CHECKS) if (!out.some((o) => o.value === value)) out.push({ value, hint: 'built-in' });
		return out;
	}

	/** Values written for `key` in objects of `kind` (lists: their items). */
	used(kind: Kind, key: string): string[] {
		return uniq(this.of(kind).flatMap((o) => scalars(o.fields[key])));
	}

	nodes(): string[] {
		return this.used('host', 'node');
	}

	/** Service names: services on hosts, service objects, host services, checks. */
	serviceNames(): Option[] {
		const out = new Map<string, string>();
		for (const s of this.services) out.set(s, 'on hosts');
		for (const o of this.of('service')) out.set(o.name, 'service');
		for (const o of this.of('host')) {
			if (isTable(o.fields.service)) for (const s of Object.keys(o.fields.service)) if (!out.has(s)) out.set(s, `host ${o.name}`);
		}
		for (const c of this.of('check')) if (!out.has(c.name)) out.set(c.name, 'check');
		return [...out].sort(([a], [b]) => a.localeCompare(b)).map(([value, hint]) => ({ value, hint }));
	}

	/** Names of scalar vars. */
	varNames(): string[] {
		return uniq(
			this.objects.flatMap((o) =>
				isTable(o.fields.vars) ? Object.entries(o.fields.vars).filter(([, v]) => !isTable(v)).map(([k]) => k) : []
			)
		);
	}

	/** Dotted paths of vars holding a table per item (`for_each` data), nested ones too. */
	forEachPaths(): string[] {
		const paths = (t: Record<string, unknown>, prefix: string): string[] =>
			Object.entries(t).flatMap(([k, v]) => {
				if (!isTable(v)) return [];
				const path = prefix + k;
				const items = Object.values(v);
				const here = items.length > 0 && items.every(isTable) ? [path] : [];
				return [...here, ...paths(v, path + '.')];
			});
		return uniq(this.objects.flatMap((o) => (isTable(o.fields.vars) ? paths(o.fields.vars, '') : [])));
	}

	/** Values written for var `name` anywhere. */
	varValues(name: string): string[] {
		return uniq(this.objects.flatMap((o) => (isTable(o.fields.vars) ? scalars(o.fields.vars[name]) : [])));
	}

	metrics(): string[] {
		return uniq(this.objects.flatMap((o) => (isTable(o.fields.thresholds) ? Object.keys(o.fields.thresholds) : [])));
	}

	thresholds(): string[] {
		return uniq(
			this.objects.flatMap((o) =>
				isTable(o.fields.thresholds)
					? Object.values(o.fields.thresholds).flatMap((t) => (isTable(t) ? scalars(t.warn).concat(scalars(t.crit)) : []))
					: []
			)
		);
	}

	commands(kind: Kind): Option[] {
		return [
			...this.used(kind, 'command').map((value) => ({ value, hint: 'used' })),
			...this.scripts.map((s) => ({ value: s.name, hint: s.dir }))
		];
	}

	/** Options for a form field of `kind`. */
	field(kind: Kind, f: Field): Option[] {
		if (f.suggest) return this.names(f.suggest);
		if (f.suggestOne === 'check') return this.checkNames();
		if (f.suggestOne) return this.names(f.suggestOne);
		const used = this.used(kind, f.key);
		switch (f.key) {
			case 'name':
				return [];
			case 'command':
				return this.commands(kind);
			case 'for_each':
				return this.forEachPaths().map((value) => ({ value }));
			case 'node':
				return this.nodes().map((value) => ({ value }));
			case 'host_check':
			case 'graph':
				return this.serviceNames();
			case 'states':
				return STATES.map((value) => ({ value }));
			case 'on':
				return ON.map((value) => ({ value }));
			case 'interval':
			case 'retry_interval':
			case 'timeout':
			case 'freshness':
				return byDuration(uniq([...used, ...DURATIONS])).map((value) => ({ value }));
			case 'email':
				return this.used('contact', 'email').map((value) => ({ value }));
			default:
				return used.map((value) => ({ value }));
		}
	}

	/** Options for a selector input, by its key (`hosts.groups`, ...). */
	selector(selects: 'host' | 'service', key: string): Option[] {
		const hosts = () => this.names('host');
		switch (key) {
			case 'name':
				return selects === 'host' ? hosts() : this.serviceNames();
			case 'address':
				return this.used('host', 'address').map((value) => ({ value }));
			case 'node':
				return this.nodes().map((value) => ({ value }));
			case 'groups':
				return this.names(selects === 'host' ? 'hostgroup' : 'servicegroup');
			case 'hosts.groups':
				return this.names('hostgroup');
			case 'checks':
				return this.names('check');
			case 'hosts.name':
				return hosts();
			default:
				return [];
		}
	}
}
