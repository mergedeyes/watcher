// The edit lease on a config file while a page edits it (DESIGN §4.1).
// Renewed every 30 seconds; the server drops it 2 minutes after the last
// renewal, so a closed tab frees the file even if the release is lost.

import { api, ApiError, heldLeases, type Lock } from './api.ts';

const RENEW_MS = 30_000;
/** While someone else has the file: how often to look whether it is free. */
const WAIT_MS = 15_000;

export class FileLease {
	/** The lease while this page holds it. */
	id = $state<string | null>(null);
	/** Who has the file while this page doesn't. */
	holder = $state<Lock | null>(null);
	/** Editors that have the file open as well (after taking it over). */
	others = $state<Lock[]>([]);
	/** This page had the lease and someone took it over. */
	lost = $state(false);
	error = $state<string | null>(null);

	#path: string;
	#timer: ReturnType<typeof setTimeout> | undefined;
	#stopped = false;
	/** Called when the lease is taken after waiting: the file may have changed. */
	#onfree: () => void;
	#unload = () => this.stop();

	constructor(path: string, onfree: () => void = () => {}) {
		this.#path = path;
		this.#onfree = onfree;
		addEventListener('pagehide', this.#unload);
		this.#tick(false);
	}

	get held() {
		return this.id !== null;
	}

	/** Edit anyway: take the file over from whoever has it. */
	takeOver() {
		clearTimeout(this.#timer);
		this.#tick(true);
	}

	async #tick(force: boolean) {
		if (this.#stopped) return;
		const waited = this.holder !== null;
		try {
			const res = await api.lock(this.#path, this.id, force);
			if (this.#stopped) {
				api.unlock(this.#path, res.lease);
				return;
			}
			this.#set(res.lease);
			this.holder = null;
			this.lost = false;
			this.others = res.others;
			this.error = null;
			if (waited && !force) this.#onfree();
		} catch (e) {
			if (e instanceof ApiError && e.status === 423) {
				if (this.id !== null) this.lost = true;
				this.#set(null);
				this.holder = e.body.holder as Lock;
			} else {
				// Keep what we have; the next renewal tries again.
				this.error = e instanceof Error ? e.message : String(e);
			}
		}
		if (!this.#stopped) this.#timer = setTimeout(() => this.#tick(false), this.held ? RENEW_MS : WAIT_MS);
	}

	#set(id: string | null) {
		if (this.id) heldLeases.delete(this.id);
		this.id = id;
		if (id) heldLeases.add(id);
	}

	/** Gives the lease up: when the page closes or edits something else. */
	stop() {
		this.#stopped = true;
		clearTimeout(this.#timer);
		removeEventListener('pagehide', this.#unload);
		if (this.id) api.unlock(this.#path, this.id);
		this.#set(null);
	}
}
