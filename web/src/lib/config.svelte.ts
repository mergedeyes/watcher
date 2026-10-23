// The object files and objects, for the Configuration pages. Loaded again
// after the server reloads its config.

import { api, type ConfigObject, type ConfigOverview, type Kind, type Problem } from './api.ts';

class ConfigState {
	overview = $state<ConfigOverview | null>(null);
	objects = $state<ConfigObject[]>([]);
	/** Files that don't parse; their objects are missing from `objects`. */
	broken = $state<Problem[]>([]);
	error = $state<string | null>(null);

	get loaded() {
		return this.overview !== null;
	}

	get writable() {
		return this.overview?.writable ?? false;
	}

	async load() {
		try {
			const [overview, objects] = await Promise.all([api.config(), api.configObjects()]);
			this.overview = overview;
			this.objects = objects.objects;
			this.broken = objects.diagnostics;
			this.error = null;
		} catch (e) {
			this.error = e instanceof Error ? e.message : String(e);
		}
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
