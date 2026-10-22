// Who is logged in, and whether this server needs a login at all.

import { api, type AuthStatus } from './api.ts';

class Auth {
	status = $state<AuthStatus | null>(null);

	/** The app may load data: logged in, or no login needed. */
	get allowed() {
		return this.status !== null && (!this.status.required || this.status.user !== null);
	}

	/** May edit the config and see Config Trees: an admin, or anyone without login. */
	get admin() {
		return this.status !== null && (!this.status.required || this.status.user?.admin === true);
	}

	async refresh() {
		this.status = await api.authStatus();
	}

	/** The session ended (401 from the API): forget the user. */
	expire() {
		if (this.status?.user) this.status = { ...this.status, user: null };
	}
}

export const auth = new Auth();
