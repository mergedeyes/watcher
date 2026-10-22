<!--
	TOML editor (CodeMirror 6) with problem markers. With `original` set,
	differences to it are shown inline and can be reverted per chunk.
-->
<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import { EditorView, basicSetup } from 'codemirror';
	import { Compartment, EditorState } from '@codemirror/state';
	import { HighlightStyle, StreamLanguage, syntaxHighlighting } from '@codemirror/language';
	import { toml } from '@codemirror/legacy-modes/mode/toml';
	import { lintGutter, setDiagnostics, type Diagnostic } from '@codemirror/lint';
	import { unifiedMergeView } from '@codemirror/merge';
	import { tags } from '@lezer/highlight';
	import type { Problem } from './api.ts';

	interface Props {
		value: string;
		readonly?: boolean;
		/** Problems for this file; lines are 1-based. */
		problems?: Problem[];
		/** Text to compare with, e.g. the file as changed by someone else. */
		original?: string | null;
		/** Line to show first. */
		line?: number;
	}

	let { value = $bindable(), readonly = false, problems = [], original = null, line = 0 }: Props = $props();

	let box: HTMLDivElement;
	let view: EditorView | null = null;
	const editable = new Compartment();
	const merge = new Compartment();

	const highlight = HighlightStyle.define([
		{ tag: tags.comment, color: 'var(--muted)', fontStyle: 'italic' },
		{ tag: [tags.string, tags.special(tags.string)], color: 'var(--cm-string)' },
		{ tag: [tags.number, tags.bool, tags.atom], color: 'var(--cm-number)' },
		{ tag: [tags.heading, tags.keyword, tags.typeName], color: 'var(--cm-header)', fontWeight: '600' },
		{ tag: [tags.propertyName, tags.variableName, tags.attributeName], color: 'var(--text)' }
	]);

	const theme = EditorView.theme({
		'&': { fontSize: '12.5px', background: 'var(--panel)', color: 'var(--text)', height: '100%' },
		'.cm-content': { fontFamily: 'var(--mono)', caretColor: 'var(--text)' },
		'.cm-scroller': { fontFamily: 'var(--mono)', lineHeight: '1.5' },
		'.cm-gutters': { background: 'var(--panel-alt)', color: 'var(--muted)', borderRight: '1px solid var(--line)' },
		'.cm-activeLine': { background: 'color-mix(in srgb, var(--link) 6%, transparent)' },
		'.cm-activeLineGutter': { background: 'color-mix(in srgb, var(--link) 10%, transparent)' },
		'&.cm-focused': { outline: 'none' },
		'.cm-selectionBackground, &.cm-focused .cm-selectionBackground, ::selection': {
			background: 'color-mix(in srgb, var(--link) 25%, transparent) !important'
		},
		'.cm-cursor': { borderLeftColor: 'var(--text)' },
		'.cm-tooltip': { background: 'var(--panel)', border: '1px solid var(--line)', color: 'var(--text)' }
	});

	function mergeExtension(text: string | null) {
		return text === null ? [] : unifiedMergeView({ original: text, mergeControls: true, gutter: true });
	}

	function markers(state: EditorState, list: Problem[]): Diagnostic[] {
		return list.map((p) => {
			const n = Math.min(Math.max(p.line, 1), state.doc.lines);
			const l = state.doc.line(n);
			return { from: l.from, to: l.to, severity: p.severity, message: p.message };
		});
	}

	onMount(() => {
		view = new EditorView({
			parent: box,
			state: EditorState.create({
				doc: value,
				extensions: [
					basicSetup,
					StreamLanguage.define(toml),
					syntaxHighlighting(highlight),
					lintGutter(),
					theme,
					EditorState.tabSize.of(2),
					editable.of([EditorState.readOnly.of(readonly), EditorView.editable.of(!readonly)]),
					merge.of(mergeExtension(original)),
					EditorView.updateListener.of((u) => {
						if (u.docChanged) value = u.state.doc.toString();
					})
				]
			})
		});
		if (line > 0) goto(line);
	});

	onDestroy(() => view?.destroy());

	export function goto(n: number) {
		if (!view) return;
		const l = view.state.doc.line(Math.min(Math.max(n, 1), view.state.doc.lines));
		view.dispatch({ selection: { anchor: l.from }, effects: EditorView.scrollIntoView(l.from, { y: 'center' }) });
		view.focus();
	}

	// Outside changes to the text (e.g. loading the file again).
	$effect(() => {
		const text = value;
		if (view && text !== view.state.doc.toString()) {
			view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: text } });
		}
	});

	$effect(() => {
		const list = problems;
		if (view) view.dispatch(setDiagnostics(view.state, markers(view.state, list)));
	});

	$effect(() => {
		const ro = readonly;
		view?.dispatch({ effects: editable.reconfigure([EditorState.readOnly.of(ro), EditorView.editable.of(!ro)]) });
	});

	$effect(() => {
		const text = original;
		view?.dispatch({ effects: merge.reconfigure(mergeExtension(text)) });
	});
</script>

<div class="editor" bind:this={box}></div>

<style>
	.editor {
		border: 1px solid var(--line);
		border-radius: var(--radius);
		overflow: hidden;
		height: 100%;
		min-height: 320px;
	}

	.editor :global(.cm-editor) {
		height: 100%;
	}
</style>
