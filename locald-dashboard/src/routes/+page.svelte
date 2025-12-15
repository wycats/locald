<script lang="ts">
	import { onMount } from 'svelte';
	import { services } from '$lib/stores/services';
	import { connectEvents } from '$lib/api';
	import Rack from '$lib/components/Rack.svelte';
	import Stream from '$lib/components/Stream.svelte';
	import Deck from '$lib/components/Deck.svelte';

	// --- State ---
	let pinned: string[] = [];

	onMount(() => {
		services.refresh();
		const cleanup = connectEvents();
		return cleanup;
	});

	$: isDeckMode = pinned.length > 0;
</script>

<div class="workspace">
	<!-- THE RACK (Sidebar) -->
	<Rack bind:pinned />

	<!-- MAIN VIEW -->
	<div class="main-view">
		{#if isDeckMode}
			<!-- THE DECK (Tiled Terminals) -->
			<Deck bind:pinned />
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
