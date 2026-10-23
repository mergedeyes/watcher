// Commands on hosts and services (re-check, acknowledge, mute, checks,
// downtime, passive result): the entries for a target, the right-click
// menu and the dialog asking for a comment or a time. ActionLayer renders
// the menu, dialog and messages.

import { api, type Action, type ActionBody, type HostView, type ServiceView } from './api.ts';

export type Target = { host: HostView; service?: undefined } | { service: ServiceView; host?: undefined };

/** A menu entry: an action, or the passive result dialog. */
export interface Entry {
	label: string;
	action: Action | 'submit';
	/** Asks for a comment, a time or a result first. */
	dialog: boolean;
}

export function label(t: Target): string {
	return t.service ? `${t.service.host}/${t.service.service}` : t.host.name;
}

export function body(t: Target): ActionBody {
	return t.service ? { services: [`${t.service.host}/${t.service.service}`] } : { hosts: [t.host.name] };
}

/** The commands for a host or service, as in Nagios. */
export function entries(t: Target): Entry[] {
	const out: Entry[] = [];
	if (t.service) {
		const s = t.service;
		if (s.passive) out.push({ label: 'Submit Result', action: 'submit', dialog: true });
		else out.push({ label: 'Re-Check Now', action: 'recheck', dialog: false });
		if (s.acknowledged) out.push({ label: 'Remove Acknowledgement', action: 'unack', dialog: false });
		else if (s.state !== null && s.state !== 'ok') out.push({ label: 'Acknowledge Problem', action: 'ack', dialog: true });
		out.push({ label: 'Schedule Downtime', action: 'downtime', dialog: true });
		out.push(s.muted ? { label: 'Unmute Notifications', action: 'unmute', dialog: false } : { label: 'Mute Notifications', action: 'mute', dialog: true });
		if (!s.passive)
			out.push(
				s.checks_enabled
					? { label: 'Disable Active Checks', action: 'disable-checks', dialog: true }
					: { label: 'Enable Active Checks', action: 'enable-checks', dialog: false }
			);
	} else {
		const h = t.host;
		out.push({ label: 'Re-Check All Services', action: 'recheck', dialog: false });
		if (h.acknowledged) out.push({ label: 'Remove Acknowledgement', action: 'unack', dialog: false });
		else if (h.state === 'down') out.push({ label: 'Acknowledge Host Problem', action: 'ack', dialog: true });
		out.push({ label: 'Schedule Downtime', action: 'downtime', dialog: true });
		out.push(
			h.muted
				? { label: 'Unmute Notifications', action: 'unmute', dialog: false }
				: { label: 'Mute Notifications', action: 'mute', dialog: true }
		);
		out.push(
			h.checks_enabled
				? { label: 'Disable Active Checks', action: 'disable-checks', dialog: true }
				: { label: 'Enable Active Checks', action: 'enable-checks', dialog: false }
		);
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

class Actions {
	/** The open right-click menu. */
	menu = $state<{ x: number; y: number; target: Target } | null>(null);
	/** The open dialog. */
	dialog = $state<{ entry: Entry; target: Target } | null>(null);
	/** A short result, shown for a few seconds. */
	message = $state<{ text: string; error: boolean } | null>(null);
	#timer: ReturnType<typeof setTimeout> | undefined;

	open(e: MouseEvent, target: Target) {
		e.preventDefault();
		this.menu = { x: e.clientX, y: e.clientY, target };
	}

	/** Runs an entry, or opens its dialog. */
	choose(entry: Entry, target: Target) {
		this.menu = null;
		if (entry.dialog) this.dialog = { entry, target };
		else if (entry.action !== 'submit') void this.run(entry.action, body(target));
	}

	async run(action: Action, b: ActionBody): Promise<boolean> {
		try {
			const r = await api.action(action, b);
			const skipped = r.skipped.map((s) => `${s.target}: ${s.reason}`);
			if (r.done.length === 0 && skipped.length) this.say(`Skipped: ${skipped.join(', ')}`, true);
			else this.say(`${DONE[action]}: ${r.done.join(', ')}${skipped.length ? ` (skipped ${skipped.join(', ')})` : ''}`);
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
