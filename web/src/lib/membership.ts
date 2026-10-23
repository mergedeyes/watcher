// Group membership written on either side: a host lists its host groups,
// or a host group lists the host (by name or pattern); the same for
// service and contact groups. Forms show both sides, so nobody has to
// remember where a membership was added.

import type { ConfigObject, Kind } from './api.ts';

/** A list field and the field on the other side that holds the same relation. */
export interface Relation {
	field: string;
	other: Kind;
	otherField: string;
	/** The other side may hold patterns (`web*`): host group members. */
	patterns?: boolean;
}

export const RELATIONS: Partial<Record<Kind, Relation>> = {
	host: { field: 'groups', other: 'hostgroup', otherField: 'members' },
	hostgroup: { field: 'members', other: 'host', otherField: 'groups', patterns: true },
	service: { field: 'groups', other: 'servicegroup', otherField: 'services' },
	servicegroup: { field: 'services', other: 'service', otherField: 'groups' },
	contact: { field: 'groups', other: 'contactgroup', otherField: 'members' },
	contactgroup: { field: 'members', other: 'contact', otherField: 'groups' }
};

/** A membership written on the other side. */
export interface Remote {
	/** The other object (a group, or a member). */
	name: string;
	/** Listed by name there: it can be removed from here. */
	listed: boolean;
	/** The pattern on the host group that matches, e.g. `web*`. */
	pattern?: string;
}

export function list(v: unknown): string[] {
	if (v === undefined || v === null) return [];
	return Array.isArray(v) ? v.map(String) : [String(v)];
}

export function glob(pattern: string): RegExp {
	const re = pattern.replace(/[.+^${}()|[\]\\]/g, '\\$&').replace(/\*/g, '.*').replace(/\?/g, '.');
	return new RegExp(`^${re}$`);
}

const isPattern = (s: string) => /[*?]/.test(s);

/**
 * Memberships of `name` (an object of `kind`) written on the other side,
 * without those it lists itself (`own`).
 */
export function remote(objects: ConfigObject[], kind: Kind, name: string, own: string[]): Remote[] {
	const rel = RELATIONS[kind];
	if (!rel || !name) return [];
	const out: Remote[] = [];
	for (const o of objects) {
		if (o.kind !== rel.other || own.includes(o.name)) continue;
		const there = list(o.fields[rel.otherField]);
		if (there.includes(name)) out.push({ name: o.name, listed: true });
		else if (kind === 'host') {
			// Host group members may be patterns.
			const p = there.find((m) => isPattern(m) && glob(m).test(name));
			if (p) out.push({ name: o.name, listed: false, pattern: p });
		}
	}
	return out.sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true }));
}

/** Fields of `other` with `name` removed from its relation list. */
export function without(other: ConfigObject, field: string, name: string): Record<string, unknown> {
	const fields = { ...other.fields };
	const rest = list(fields[field]).filter((x) => x !== name);
	if (rest.length) fields[field] = rest;
	else delete fields[field];
	return fields;
}
