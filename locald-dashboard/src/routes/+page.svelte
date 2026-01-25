<script lang="ts">
	import { onMount } from 'svelte';
	import { services } from '$lib/stores/services';
	import { connectEvents } from '$lib/api';
	import Rack from '$lib/components/Rack.svelte';
	import Stream from '$lib/components/Stream.svelte';
	import Deck from '$lib/components/Deck.svelte';

	// --- State ---
	let monitored = $state<string[]>([]);

	onMount(() => {
		const params = new URLSearchParams(window.location.search);
		const monitorParams = params.getAll('monitor');
		const monitors = monitorParams
			.flatMap((value) => value.split(','))
			.map((value) => value.trim())
			.filter(Boolean);

		if (monitors.length > 0) {
			monitored = Array.from(new Set(monitors));
		}

		services.refresh();
		const cleanup = connectEvents();
		return cleanup;
	});

	let isDeckMode = $derived(monitored.length > 0);
</script>

<div class="workspace">
	<!-- THE RACK (Sidebar) -->
	<Rack bind:monitored />

	<!-- MAIN VIEW -->
	<div class="main-view">
		{#if isDeckMode}
			<!-- THE DECK (Tiled Terminals) -->
			<Deck bind:monitored />
		{:else}
			<!-- THE STREAM (Unified Log) -->
			<Stream />
		{/if}
	</div>
</div>

<style>
	.workspace {
		display: grid;
		grid-template-columns: 280px 1fr;
		height: 100vh;
		width: 100vw;
	}

	.main-view {
		background: #09090b;
		display: flex;
		flex-direction: column;
		overflow: hidden;
	}
</style>
