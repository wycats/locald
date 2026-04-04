<script lang="ts">
	import { onMount } from 'svelte';
	import { services } from '$lib/stores/services';
	import { projectList } from '$lib/stores/projects';
	import { connectEvents } from '$lib/api';
	import Rack from '$lib/components/Rack.svelte';
	import Stream from '$lib/components/Stream.svelte';
	import Deck from '$lib/components/Deck.svelte';
	import ProjectView from '$lib/components/ProjectView.svelte';

	// --- State ---
	let monitored = $state<string[]>([]);
	let selectedProject = $state<string | null>(null);

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
		projectList.refresh();
		const cleanup = connectEvents();
		return cleanup;
	});

	let isDeckMode = $derived(monitored.length > 0);

	function handleSelectProject(path: string | null) {
		selectedProject = path;
	}
</script>

<div class="workspace">
	<!-- THE RACK -->
	<Rack bind:monitored {selectedProject} onSelectProject={handleSelectProject} />

	<!-- MAIN VIEW -->
	<div class="main-view">
		{#if selectedProject}
			<ProjectView projectPath={selectedProject} />
		{:else if isDeckMode}
			<Deck bind:monitored />
		{:else}
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

	@media (max-width: 640px) {
		.workspace {
			grid-template-columns: 1fr;
		}
	}
</style>
