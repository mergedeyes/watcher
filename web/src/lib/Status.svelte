<!-- Status badge: solid state colour, as in the Nagios Status column. -->
<script lang="ts">
	import type { State } from './api.ts';
	import { stateName } from './format.ts';

	let { state }: { state: State | null } = $props();
</script>

<span class="st" data-state={state ?? 'pending'}>{stateName(state)}</span>
