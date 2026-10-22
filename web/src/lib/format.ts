import type { State } from './api.ts';

const STATE_NAMES: Record<State, string> = {
	ok: 'OK',
	warning: 'WARNING',
	critical: 'CRITICAL',
	unknown: 'UNKNOWN'
};

export function stateName(state: State | null): string {
	return state ? STATE_NAMES[state] : 'PENDING';
}

/** Severity for sorting: CRITICAL first, as in the hub (OK < WARNING < UNKNOWN < CRITICAL). */
export function severity(state: State | null): number {
	switch (state) {
		case 'critical':
			return 4;
		case 'unknown':
			return 3;
		case 'warning':
			return 2;
		case 'ok':
			return 0;
		default:
			return 1;
	}
}

function trim(n: number): string {
	return Number(n.toFixed(3)).toString();
}

/** A normalized value for humans, like the hub: `12 ms`, `1.5 GiB`, `56%`. */
export function formatValue(value: number, unit = ''): string {
	switch (unit) {
		case '':
		case 'c':
			return trim(value);
		case '%':
			return `${trim(value)}%`;
		case 's': {
			const abs = Math.abs(value);
			if (abs !== 0 && abs < 1e-3) return `${trim(value * 1e6)} µs`;
			if (abs !== 0 && abs < 1) return `${trim(value * 1e3)} ms`;
			return `${trim(value)} s`;
		}
		case 'B': {
			const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB', 'PiB'];
			let v = value;
			let i = 0;
			while (Math.abs(v) >= 1024 && i < units.length - 1) {
				v /= 1024;
				i++;
			}
			return `${trim(v)} ${units[i]}`;
		}
		default:
			return `${trim(value)}${unit}`;
	}
}

/**
 * A threshold as the hub sends it (bounds in the stored unit, e.g. `> 0.5`
 * for seconds), with its numbers formatted for humans: `> 500 ms`.
 */
export function formatThreshold(text: string | undefined, unit = ''): string {
	if (!text) return '';
	return text.replace(/-?\d+(?:\.\d+)?(?:e[+-]?\d+)?/gi, (n) => formatValue(Number(n), unit));
}

/** `40s`, `12m`, `3h`, `2d`: compact age for windows and tables. */
export function age(iso: string | null, now: number): string {
	if (!iso) return '–';
	const secs = Math.max(0, Math.round((now - Date.parse(iso)) / 1000));
	if (secs < 60) return `${secs}s`;
	if (secs < 3600) return `${Math.floor(secs / 60)}m`;
	if (secs < 86400) return `${Math.floor(secs / 3600)}h`;
	return `${Math.floor(secs / 86400)}d`;
}

/** Local date and time, to the second. */
export function timestamp(iso: string | null): string {
	if (!iso) return '–';
	return new Date(iso).toLocaleString(undefined, {
		year: 'numeric',
		month: 'short',
		day: 'numeric',
		hour: '2-digit',
		minute: '2-digit',
		second: '2-digit'
	});
}

export function duration(secs: number): string {
	if (secs % 86400 === 0) return `${secs / 86400}d`;
	if (secs % 3600 === 0) return `${secs / 3600}h`;
	if (secs % 60 === 0) return `${secs / 60}m`;
	return `${secs}s`;
}

/** `temp{sensor="cpu0"}` */
export function seriesName(metric: string, labels: Record<string, string> | undefined): string {
	const entries = Object.entries(labels ?? {});
	if (entries.length === 0) return metric;
	return `${metric}{${entries.map(([k, v]) => `${k}="${v}"`).join(',')}}`;
}

/** Nagios-style duration since `iso`: `0d 2h 13m 5s`. */
export function since(iso: string | null, now: number): string {
	if (!iso) return '';
	let secs = Math.max(0, Math.floor((now - Date.parse(iso)) / 1000));
	const d = Math.floor(secs / 86400);
	secs %= 86400;
	const h = Math.floor(secs / 3600);
	secs %= 3600;
	const m = Math.floor(secs / 60);
	return `${d}d ${h}h ${m}m ${secs % 60}s`;
}

/** `2026-10-22 09:31:47`, local time. */
export function datetime(iso: string | null): string {
	if (!iso) return '';
	const t = new Date(iso);
	const p = (n: number) => String(n).padStart(2, '0');
	return `${t.getFullYear()}-${p(t.getMonth() + 1)}-${p(t.getDate())} ${p(t.getHours())}:${p(t.getMinutes())}:${p(t.getSeconds())}`;
}
