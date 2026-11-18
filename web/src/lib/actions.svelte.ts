// Commands on hosts and services (re-check, acknowledge, mute, checks,
// downtime, passive result): the entries for one or several targets, the
// right-click menu and the dialog asking for a comment or a time.
// ActionLayer renders the menu, dialog and messages.

import { api, type Action, type ActionBody, type HostView, type ServiceView } from './api.ts';
import type { Selection } from './selection.svelte.ts';

export type Target = { host: HostView; service?: undefined } | { service: ServiceView; host?: undefined };

/** A menu entry: an action, or the passive result dialog. */
export interface Entry {
	label: string;
	/** For the selection bar. */
	short: string;
	action: Action | 'submit';
	/** Asks for a comment, a time or a result first. */
	dialog: boolean;
	/** The targets it applies to, e.g. only the unmuted ones for Mute. */
	targets: Target[];
}

/** `host` or `host/service`. */
export function id(t: Target): string {
	return t.service ? `${t.service.host}/${t.service.service}` : t.host.name;
}

/** `web01/HTTP`, or `3 services`. */
export function label(targets: Target[]): string {
	if (targets.length === 1 && targets[0]) return id(targets[0]);
	const services = targets.filter((t) => t.service).length;
	const hosts = targets.length - services;
	const n = (k: number, one: string) => (k ? [`${k} ${one}${k === 1 ? '' : 's'}`] : []);
	return [...n(hosts, 'host'), ...n(services, 'service')].join(', ');
}

export function body(targets: Target[]): ActionBody {
	const hosts = targets.filter((t) => t.host).map(id);
	const services = targets.filter((t) => t.service).map(id);
	return { ...(hosts.length ? { hosts } : {}), ...(services.length ? { services } : {}) };
}

/** Whether a command makes sense for a target, as in Nagios. */
function applies(action: Action | 'submit', t: Target): boolean {
	if (t.service) {
		const s = t.service;
		switch (action) {
			case 'submit':
				return s.passive;
			case 'recheck':
				return !s.passive;
			case 'ack':
				return !s.acknowledged && s.state !== null && s.state !== 'ok';
			case 'unack':
				return s.acknowledged;
			case 'downtime':
				return true;
			case 'mute':
				return !s.muted;
			case 'unmute':
				return s.muted;
			case 'disable-checks':
				return !s.passive && s.checks_enabled;
			case 'enable-checks':
				return !s.passive && !s.checks_enabled;
		}
	}
	const h = t.host;
	switch (action) {
		case 'submit':
			return false;
		case 'ack':
			return !h.acknowledged && h.state === 'down';
		case 'unack':
			return h.acknowledged;
		case 'mute':
			return !h.muted;
		case 'unmute':
			return h.muted;
		case 'disable-checks':
			return h.checks_enabled;
		case 'enable-checks':
			return !h.checks_enabled;
		default:
			return true;
	}
}

/** In menu order: [action, label, short label, asks first]. */
const COMMANDS: [Action | 'submit', string, string, boolean][] = [
	['submit', 'Submit Result', 'Submit Result', true],
	['recheck', 'Re-Check Now', 'Re-Check', false],
	['ack', 'Acknowledge Problem', 'Acknowledge', true],
	['unack', 'Remove Acknowledgement', 'Remove Ack', false],
	['downtime', 'Schedule Downtime', 'Downtime', true],
	['mute', 'Mute Notifications', 'Mute', true],
	['unmute', 'Unmute Notifications', 'Unmute', false],
	['disable-checks', 'Disable Active Checks', 'Disable Checks', true],
	['enable-checks', 'Enable Active Checks', 'Enable Checks', false]
];

/** The commands that apply to at least one of the targets. */
export function entries(targets: Target[]): Entry[] {
	const many = targets.length > 1;
	const hosts = targets.every((t) => t.host);
	const out: Entry[] = [];
	for (const [action, name, short, dialog] of COMMANDS) {
		// One result for many services is rarely what anyone wants.
		if (action === 'submit' && many) continue;
		const to = targets.filter((t) => applies(action, t));
		if (to.length === 0) continue;
		let text = name;
		if (action === 'recheck' && hosts) text = 'Re-Check All Services';
		else if (action === 'ack') text = `Acknowledge ${hosts ? 'Host ' : ''}Problem${many ? 's' : ''}`;
		else if (action === 'unack' && many) text = 'Remove Acknowledgements';
		out.push({ label: text, short, action, dialog, targets: to });
	}
	return out;
}

const DONE: Record<Action, string> = {
	recheck: 'Re-check scheduled',
	ack: 'Acknowledged',
	unack: 'Acknowledgement removed',
	mute: 'Muted',
	unmute: 'Unmuted',
	'disable-checks': 'Active checks disabled',
	'enable-checks': 'Active checks enabled',
	downtime: 'Downtime scheduled'
};

/** `a, b, c +12 more`: a command on a whole page names a few targets only. */
function some(names: string[]): string {
	const SHOWN = 3;
	return names.length <= SHOWN + 1 ? names.join(', ') : `${names.slice(0, SHOWN).join(', ')} +${names.length - SHOWN} more`;
}

class Actions {
	/** The open right-click menu, and the selection it came from. */
	menu = $state<{ x: number; y: number; targets: Target[]; selection?: Selection } | null>(null);
	/** The open dialog. */
	dialog = $state<{ entry: Entry; selection?: Selection } | null>(null);
	/** A short result, shown for a few seconds. */
	message = $state<{ text: string; error: boolean } | null>(null);
	#timer: ReturnType<typeof setTimeout> | undefined;

	/** Opens the menu for a row; for all selected rows if it is one of them. */
	open(e: MouseEvent, target: Target, selection?: Selection) {
		e.preventDefault();
		const all = selection?.has(target) ? selection.targets : [];
		this.menu =
			all.length > 1
				? { x: e.clientX, y: e.clientY, targets: all, selection }
				: { x: e.clientX, y: e.clientY, targets: [target] };
	}

	/** Runs an entry, or opens its dialog. A selection is cleared once it ran. */
	choose(entry: Entry, selection?: Selection) {
		this.menu = null;
		if (entry.dialog) this.dialog = { entry, selection };
		else if (entry.action !== 'submit') void this.run(entry.action, body(entry.targets), selection);
	}

	async run(action: Action, b: ActionBody, selection?: Selection): Promise<boolean> {
		try {
			const r = await api.action(action, b);
			const skipped = r.skipped.map((s) => `${s.target}: ${s.reason}`);
			if (r.done.length === 0 && skipped.length) this.say(`Skipped: ${some(skipped)}`, true);
			else this.say(`${DONE[action]}: ${some(r.done)}${skipped.length ? ` (skipped ${some(skipped)})` : ''}`);
			selection?.clear();
			return true;
		} catch (e) {
			this.say(e instanceof Error ? e.message : String(e), true);
			return false;
		}
	}

	say(text: string, error = false) {
		this.message = { text, error };
		clearTimeout(this.#timer);
		this.#timer = setTimeout(() => (this.message = null), error ? 8000 : 4000);
	}
}

export const actions = new Actions();
