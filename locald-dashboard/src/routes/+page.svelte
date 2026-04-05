<script lang="ts">
	/* eslint-disable svelte/no-navigation-without-resolve */
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
	import { services } from '$lib/stores/services';
	import { projectList } from '$lib/stores/projects';
	import { connectEvents } from '$lib/api';
	import Rack from '$lib/components/Rack.svelte';
	import Stream from '$lib/components/Stream.svelte';
	import Deck from '$lib/components/Deck.svelte';
	import ProjectView from '$lib/components/ProjectView.svelte';

	// --- URL-derived state ---
	// project and service derive from the URL. monitored is mutable (Rack/Deck write to it).

	let selectedProject = $derived.by(() => {
		const name = $page.url.searchParams.get('project');
		if (!name) return null;
		const entry = $projectList.find((p) => p.project_name === name);
		return entry?.project_path ?? null;
	});

	let selectedService = $derived($page.url.searchParams.get('service'));

	let monitored = $state<string[]>([]);

	let isDeckMode = $derived(monitored.length > 0);

	// --- Navigation (writes to URL) ---

	function handleSelectProject(path: string | null) {
		if (!path) {
			goto('/', { replaceState: true });
			return;
		}
		const entry = $projectList.find((p) => p.project_path === path);
		const name = entry?.project_name ?? path.split('/').pop();
		if (name) {
			goto(`?project=${encodeURIComponent(name)}`, { replaceState: true });
		}
	}

	// --- Data loading ---

	onMount(() => {
		// Initialize monitored from URL (one-time, then mutable)
		const monitorParam = new URLSearchParams(window.location.search).get('monitor');
		if (monitorParam) {
			monitored = monitorParam
				.split(',')
				.map((v) => v.trim())
				.filter(Boolean);
		}

		services.refresh();
		projectList.refresh();
		const cleanup = connectEvents();
		return cleanup;
	});
</script>

<div class="workspace">
	<!-- THE RACK -->
	<Rack bind:monitored {selectedProject} onSelectProject={handleSelectProject} />

	<!-- MAIN VIEW -->
	<div class="main-view">
		{#if isDeckMode}
			<Deck bind:monitored />
		{:else if selectedProject}
			<ProjectView projectPath={selectedProject} initialService={selectedService} />
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
