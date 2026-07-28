<script lang="ts">
	/* eslint-disable svelte/no-navigation-without-resolve */
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
	import { services } from '$lib/stores/services';
	import { projectList } from '$lib/stores/projects';
	import { connectEvents } from '$lib/api';
	import { resolveServiceSelector } from '$lib/types';
	import Rack from '$lib/components/Rack.svelte';
	import Deck from '$lib/components/Deck.svelte';
	import Stream from '$lib/components/Stream.svelte';
	import ProjectView from '$lib/components/ProjectView.svelte';

	// --- URL-derived state ---
	// All view state derives from the URL. Helpers below write back via goto().

	let selectedProject = $derived.by(() => {
		const selector = $page.url.searchParams.get('project');
		if (!selector) return null;
		const entry =
			$projectList.find((project) => project.project_path === selector) ??
			$projectList.find((project) => project.project_name === selector);
		return entry?.project_path ?? null;
	});

	let monitored = $derived.by(() => {
		const selectors = $page.url.searchParams.get('monitor')?.split(',').filter(Boolean) ?? [];
		return [...new Set(selectors.map((selector) => resolveServiceSelector(selector, $services)))];
	});

	// --- Navigation (writes to URL) ---

	function handleSelectProject(path: string | null) {
		if (!path) {
			goto('/', { replaceState: true });
			return;
		}
		goto(`?project=${encodeURIComponent(path)}`, { replaceState: true });
	}

	function toggleMonitor(name: string | string[]) {
		const next = Array.isArray(name)
			? name
			: monitored.includes(name)
				? monitored.filter((n) => n !== name)
				: [...monitored, name];
		updateUrl({ monitor: next.length ? next.join(',') : null });
	}

	function updateUrl(changes: Record<string, string | null>) {
		// eslint-disable-next-line svelte/prefer-svelte-reactivity
		const params = new URLSearchParams($page.url.searchParams);
		for (const [key, value] of Object.entries(changes)) {
			if (value) params.set(key, value);
			else params.delete(key);
		}
		goto(`?${params.toString()}`, { replaceState: true });
	}

	// --- Data loading ---

	onMount(() => {
		services.refresh();
		let refreshProjectsInFlight: Promise<void> | null = null;
		const refreshProjects = () => {
			if (refreshProjectsInFlight) return refreshProjectsInFlight;
			refreshProjectsInFlight = projectList.refresh().finally(() => {
				refreshProjectsInFlight = null;
			});
			return refreshProjectsInFlight;
		};
		void refreshProjects();
		const cleanupEvents = connectEvents(refreshProjects);
		const refreshInterval = window.setInterval(refreshProjects, 30_000);
		window.addEventListener('focus', refreshProjects);
		return () => {
			cleanupEvents();
			window.clearInterval(refreshInterval);
			window.removeEventListener('focus', refreshProjects);
		};
	});
</script>

<div class="workspace">
	<!-- THE RACK -->
	<Rack
		{monitored}
		{selectedProject}
		onSelectProject={handleSelectProject}
		onToggleMonitor={toggleMonitor}
	/>

	<!-- MAIN VIEW -->
	<div class="main-view">
		{#if monitored.length > 0}
			<Deck {monitored} onToggleMonitor={toggleMonitor} />
		{:else if selectedProject}
			<ProjectView
				projectPath={selectedProject}
				{monitored}
				onToggleMonitor={toggleMonitor}
				onDeselectProject={() => handleSelectProject(null)}
			/>
		{:else}
			<Stream />
		{/if}
	</div>
</div>

<style>
	.workspace {
		display: grid;
		grid-template-columns: 280px 1fr;
		height: 100dvh;
	}

	.main-view {
		background: #09090b;
		display: flex;
		flex-direction: column;
		min-height: 0;
		min-width: 0;
		overflow: hidden;
	}

	@media (max-width: 640px) {
		.workspace {
			grid-template-columns: 1fr;
		}
	}
</style>
