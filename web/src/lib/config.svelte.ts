// The object files and objects, for the Configuration pages. Loaded again
// after the server reloads its config.

import { api, type BuiltinCheck, type ConfigObject, type ConfigOverview, type Kind, type Problem } from './api.ts';

class ConfigState {
	overview = $state<ConfigOverview | null>(null);
	objects = $state<ConfigObject[]>([]);
	/** Files that don't parse; their objects are missing from `objects`. */
	broken = $state<Problem[]>([]);
	/** The built-in checks; they don't change while the server runs. */
	builtins = $state<BuiltinCheck[]>([]);
	error = $state<string | null>(null);

	get loaded() {
		return this.overview !== null;
	}

	get writable() {
		return this.overview?.writable ?? false;
	}

	async load() {
		try {
			const [overview, objects, builtins] = await Promise.all([
				api.config(),
				api.configObjects(),
				this.builtins.length ? this.builtins : api.configBuiltins()
			]);
			this.overview = overview;
			this.builtins = builtins;
			this.objects = objects.objects;
			this.broken = objects.diagnostics;
			this.error = null;
		} catch (e) {
			this.error = e instanceof Error ? e.message : String(e);
		}
	}

	/**
	 * The built-in check behind check `name`: the built-in itself, unless a
	 * check of that name replaces it, or the built-in a check object uses.
	 */
	builtinOf(name: string): BuiltinCheck | undefined {
		const check = this.find('check', name);
		const builtin = check ? check.fields.builtin : name;
		return typeof builtin === 'string' ? this.builtins.find((b) => b.name === builtin) : undefined;
	}

	of(kind: Kind) {
		return this.objects.filter((o) => o.kind === kind);
	}

	/** The object with this name or {@link objectId}. */
	find(kind: Kind, id: string) {
		return this.objects.find((o) => o.kind === kind && objectId(o) === id);
	}
}

/** How the edit API addresses an object: its name, or `@path:line` for an unnamed notify rule. */
export function objectId(o: ConfigObject) {
	return o.name || `@${o.path}:${o.line}`;
}

export const config = new ConfigState();

/** Link to a file in the editor, optionally at a line. */
export function fileHref(path: string, line = 0) {
	return `/config/files/edit?path=${encodeURIComponent(path)}${line ? `&line=${line}` : ''}`;
}

export function editHref(kindPath: string, name?: string) {
	return `/config/${kindPath}/edit${name === undefined ? '' : `?name=${encodeURIComponent(name)}`}`;
}
