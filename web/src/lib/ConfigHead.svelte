<!-- Mode and problems shown on every Configuration page. -->
<script lang="ts">
	import { config, fileHref } from './config.svelte.ts';

	let problems = $derived([...(config.overview?.diagnostics ?? []), ...config.broken]);
	let errors = $derived(problems.filter((p) => p.severity === 'error'));
</script>

{#if config.error}
	<p class="notice error" role="alert">{config.error}</p>
{/if}
{#if config.overview && !config.overview.writable}
	<p class="notice">Read-only: <code>config_mode = "{config.overview.mode}"</code></p>
{/if}
{#if errors.length > 0}
	<div class="notice error" role="alert">
		<strong>Config on disk has errors; the running config is older.</strong>
		<ul>
			{#each errors as p, i (i)}
				<li>
					{#if p.path}<a class="mono" href={fileHref(p.path, p.line)}>{p.path}{p.line ? `:${p.line}` : ''}</a>{/if}
					{p.message}
				</li>
			{/each}
		</ul>
	</div>
{/if}

<style>
	.notice {
		margin: 0 0 12px;
	}

	.error {
		border-color: var(--crit);
		color: var(--text);
	}

	strong {
		color: var(--crit);
	}

	ul {
		margin: 6px 0 0;
		padding-left: 18px;
	}
</style>
