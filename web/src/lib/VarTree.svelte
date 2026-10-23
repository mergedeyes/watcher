<!--
	A host var: text, nested keys, or a table of entries (for_each data)
	with one row per entry and a column per key.
-->
<script lang="ts">
	import VarTree from './VarTree.svelte';

	let { v }: { v: unknown } = $props();

	type Obj = Record<string, unknown>;
	const isObj = (x: unknown): x is Obj => typeof x === 'object' && x !== null && !Array.isArray(x);
	const text = (x: unknown) => (x === undefined ? '' : typeof x === 'string' ? x : JSON.stringify(x));

	// Entries that are all flat tables.
	let rows = $derived.by(() => {
		if (!isObj(v)) return null;
		const entries = Object.values(v);
		const flat = entries.every((e) => isObj(e) && Object.values(e).every((x) => !isObj(x)));
		return entries.length && flat ? (v as Record<string, Obj>) : null;
	});
	let columns = $derived(rows ? [...new Set(Object.values(rows).flatMap((e) => Object.keys(e)))] : []);
</script>

{#if rows}
	<table class="vars">
		{#if columns.length}
			<thead>
				<tr>
					<th></th>
					{#each columns as c (c)}<th>{c}</th>{/each}
				</tr>
			</thead>
		{/if}
		<tbody>
			{#each Object.entries(rows) as [k, e] (k)}
				<tr>
					<th>{k}</th>
					{#each columns as c (c)}<td>{text(e[c])}</td>{/each}
				</tr>
			{/each}
		</tbody>
	</table>
{:else if isObj(v) && Object.keys(v).length}
	<table class="vars">
		<tbody>
			{#each Object.entries(v) as [k, x] (k)}
				<tr>
					<th>{k}</th>
					<td><VarTree v={x} /></td>
				</tr>
			{/each}
		</tbody>
	</table>
{:else if isObj(v)}
	<span class="muted">–</span>
{:else}
	{text(v)}
{/if}
