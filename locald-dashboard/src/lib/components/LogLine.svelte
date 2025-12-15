<script lang="ts">
	import Converter from 'ansi-to-html';

	export let message: string;

	// Configure the converter to use standard terminal colors
	const converter = new Converter({
		fg: '#a1a1aa', // Default foreground (Zinc-400)
		bg: '#09090b', // Default background (Zinc-950)
		newline: false,
		escapeXML: true
	});

	$: html = converter.toHtml(message);
</script>

<span class="log-msg">
	<!-- eslint-disable-next-line svelte/no-at-html-tags -->
	{@html html}
</span>

<style>
	.log-msg {
		color: #d4d4d8;
		white-space: pre-wrap;
		word-break: break-all;
		font-family: 'JetBrains Mono', monospace;
	}

	/* Ensure ANSI colors render with sufficient contrast */
	:global(.log-msg span) {
		font-weight: normal;
	}
</style>
