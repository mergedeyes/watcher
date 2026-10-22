<!-- Status badge: solid state colour, as in the Nagios Status column. -->
<script lang="ts">
	import type { HostState, State } from './api.ts';
	import { stateName } from './format.ts';

	let { state }: { state: State | HostState | null } = $props();

	// Hosts use the service colours: UP green, DOWN red.
	const COLOUR: Record<string, string> = { up: 'ok', down: 'critical' };
</script>

<span class="st" data-state={state ? (COLOUR[state] ?? state) : 'pending'}>{stateName(state)}</span>
