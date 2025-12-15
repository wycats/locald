<script lang="ts">
	import { projects } from '$lib/stores/services';
	import type { ServiceStatus } from '$lib/types';
	import {
		RotateCw,
		Square,
		Box,
		Terminal as TerminalIcon,
		Activity,
		Play,
		ExternalLink
	} from 'lucide-svelte';
	import {
		stopAllServices,
		restartAllServices,
		startService,
		stopService,
		restartService
	} from '$lib/api';

	interface Props {
		selectedProject: string | null;
		onSelectProject: (name: string | null) => void;
		onInspect: (serviceName: string) => void;
	}

	interface Project {
		name: string;
		services: ServiceStatus[];
	}

	let { selectedProject, onSelectProject, onInspect }: Props = $props();

	async function handleStopAll() {
		if (!confirm('Are you sure you want to stop all services?')) return;
		try {
			await stopAllServices();
		} catch (e) {
			alert(e instanceof Error ? e.message : String(e));
		}
	}

	async function handleRestartAll() {
		if (!confirm('Are you sure you want to restart all services?')) return;
		try {
			await restartAllServices();
		} catch (e) {
			alert(e instanceof Error ? e.message : String(e));
		}
	}

	async function handleServiceAction(
		e: Event,
		action: 'start' | 'stop' | 'restart',
		serviceName: string
	) {
		e.stopPropagation();
		try {
			if (action === 'start') await startService(serviceName);
			if (action === 'stop') await stopService(serviceName);
			if (action === 'restart') await restartService(serviceName);
		} catch (err) {
			console.error(err);
		}
	}

	function getDisplayName(service: ServiceStatus) {
		return service.name.split(':').pop();
	}

	let systemProjects = $derived($projects.filter((p) => p.name.startsWith('locald-')));
	let userProjects = $derived($projects.filter((p) => !p.name.startsWith('locald-')));
</script>

{#snippet projectGroup(project: Project)}
	<div class="project-group">
		<button
			class="nav-item project-header-btn"
			class:selected={selectedProject === project.name}
			onclick={() => onSelectProject(project.name)}
		>
			<span>{project.name}</span>
		</button>
		{#each project.services as service (service.name)}
			<!-- svelte-ignore a11y_click_events_have_key_events -->
			<!-- svelte-ignore a11y_no_static_element_interactions -->
			<div class="nav-item sub-item sidebar-item" onclick={() => onInspect(service.name)}>
				<div class="service-info">
					<span
						class="status-dot"
						class:running={service.status === 'running'}
						class:healthy={service.health_status === 'Healthy'}
						class:starting={service.health_status === 'Starting'}
						class:unhealthy={service.health_status === 'Unhealthy'}
					></span>
					<span class="name">
						{getDisplayName(service)}
					</span>
				</div>

				<div class="sidebar-actions">
					{#if service.status === 'running'}
						<button
							title="Restart"
							onclick={(e) => handleServiceAction(e, 'restart', service.name)}
						>
							<RotateCw size={12} />
						</button>
					{:else}
						<button title="Start" onclick={(e) => handleServiceAction(e, 'start', service.name)}>
							<Play size={12} />
						</button>
					{/if}

					<button
						title="Terminal"
						onclick={(e) => {
							e.stopPropagation();
							onInspect(service.name);
						}}
					>
						<TerminalIcon size={12} />
					</button>

					{#if service.url}
						<!-- eslint-disable-next-line svelte/no-navigation-without-resolve -->
						<a href={service.url} target="_blank" title="Open" onclick={(e) => e.stopPropagation()}>
							<ExternalLink size={12} />
						</a>
					{/if}
				</div>
			</div>
		{/each}
	</div>
{/snippet}

<div class="sidebar">
	<div class="header">
		<div class="brand">
			<Box size={20} />
			<span>locald</span>
		</div>
		<div class="global-controls">
			<button title="Restart All" onclick={handleRestartAll}>
				<RotateCw size={16} />
			</button>
			<button title="Stop All" onclick={handleStopAll}>
				<Square size={16} />
			</button>
		</div>
	</div>

	<div class="nav">
		<button
			class="nav-item"
			class:selected={selectedProject === null}
			onclick={() => onSelectProject(null)}
		>
			<Activity size={16} />
			<span>All Projects</span>
		</button>

		{#each userProjects as project (project.name)}
			{@render projectGroup(project)}
		{/each}

		{#if systemProjects.length > 0}
			<div class="section-divider"></div>
			<div class="section-header">System</div>
			{#each systemProjects as project (project.name)}
				{@render projectGroup(project)}
			{/each}
		{/if}
	</div>
</div>

<style>
	.sidebar {
		width: 280px;
		background: #09090b; /* Zinc-950 */
		border-right: 1px solid #27272a; /* Zinc-800 */
		display: flex;
		flex-direction: column;
		height: 100%;
	}

	.header {
		padding: 16px;
		border-bottom: 1px solid #27272a; /* Zinc-800 */
		display: flex;
		justify-content: space-between;
		align-items: center;
	}

	.brand {
		display: flex;
		align-items: center;
		gap: 8px;
		font-weight: 600;
		font-size: 1.1rem;
		color: #e4e4e7; /* Zinc-200 */
	}

	.global-controls {
		display: flex;
		gap: 4px;
	}

	.global-controls button {
		background: transparent;
		border: none;
		color: #a1a1aa; /* Zinc-400 */
		cursor: pointer;
		padding: 4px;
		border-radius: 4px;
	}
	.global-controls button:hover {
		background: #27272a; /* Zinc-800 */
		color: #e4e4e7; /* Zinc-200 */
	}

	.nav {
		flex: 1;
		overflow-y: auto;
		overflow-x: hidden;
		padding: 8px;
	}

	.nav-item {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 8px;
		padding: 8px 12px;
		width: 100%;
		background: transparent;
		border: none;
		color: #a1a1aa; /* Zinc-400 */
		cursor: pointer;
		border-radius: 6px;
		text-align: left;
		font-size: 0.9rem;
		box-sizing: border-box;
		transition:
			background-color 0.15s,
			color 0.15s;
	}

	.nav-item:hover {
		background: rgba(255, 255, 255, 0.05); /* bg-white/5 */
		color: #e4e4e7; /* Zinc-200 */
	}

	.nav-item.selected {
		background: #27272a; /* Zinc-800 */
		color: #e4e4e7; /* Zinc-200 */
		font-weight: 500;
	}

	.project-group {
		margin-top: 16px;
	}

	.project-header-btn {
		font-weight: 600;
		text-transform: uppercase;
		font-size: 0.75rem;
		color: #71717a; /* Zinc-500 */
		letter-spacing: 0.05em;
	}

	.sub-item {
		padding-left: 24px;
		cursor: default;
		height: 32px;
		display: grid;
		grid-template-columns: 1fr auto; /* Grid Stack */
		align-items: center;
	}
	.sub-item:hover {
		background: rgba(255, 255, 255, 0.05); /* bg-white/5 */
		color: #e4e4e7; /* Zinc-200 */
	}

	.service-info {
		display: flex;
		align-items: center;
		gap: 8px;
		overflow: hidden;
		min-width: 0; /* Allow truncation */
	}

	.status-dot {
		width: 6px;
		height: 6px;
		border-radius: 50%;
		background: #71717a; /* Zinc-500 */
		flex-shrink: 0;
	}
	.status-dot.running {
		background: #a1a1aa; /* Zinc-400 */
	}
	.status-dot.healthy {
		background: #4ade80; /* Green-400 */
		box-shadow: 0 0 0 1px rgba(74, 222, 128, 0.2);
	}
	.status-dot.starting {
		background: #facc15; /* Yellow-400 */
	}
	.status-dot.unhealthy {
		background: #f87171; /* Red-400 */
	}

	.name {
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.sidebar-actions {
		display: none;
		gap: 2px;
		align-items: center;
	}

	.sub-item:hover .sidebar-actions {
		display: flex;
	}

	.sidebar-actions button,
	.sidebar-actions a {
		background: transparent;
		border: none;
		color: #a1a1aa; /* Zinc-400 */
		cursor: pointer;
		padding: 4px;
		border-radius: 4px;
		display: flex;
		align-items: center;
		justify-content: center;
	}

	.sidebar-actions button:hover,
	.sidebar-actions a:hover {
		background: #3f3f46; /* Zinc-700 */
		color: #e4e4e7; /* Zinc-200 */
	}

	.section-divider {
		height: 1px;
		background: #27272a; /* Zinc-800 */
		margin: 16px 0 8px 0;
	}

	.section-header {
		padding: 0 12px;
		font-size: 0.75rem;
		text-transform: uppercase;
		color: #71717a; /* Zinc-500 */
		font-weight: 600;
		margin-bottom: 4px;
		letter-spacing: 0.05em;
	}
</style>
