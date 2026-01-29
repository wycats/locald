<script lang="ts">
	import '../app.css';
	import favicon from '$lib/assets/favicon.svg';
	import CommandPalette from '$lib/components/CommandPalette.svelte';
	import Toast from '$lib/components/Toast.svelte';

	let { children } = $props();
</script>

<svelte:head>
	<title>Locald</title>
	<link rel="icon" href={favicon} />
</svelte:head>

{@render children()}
<CommandPalette />
<Toast />
