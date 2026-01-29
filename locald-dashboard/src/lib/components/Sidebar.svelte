<script lang="ts">
	import { projects } from '$lib/stores/services';
	import { toasts } from '$lib/stores/toasts';
	import { pendingActions } from '$lib/stores/actions';
	import {
		startServiceWithFeedback,
		stopServiceWithFeedback,
		restartServiceWithFeedback
	} from '$lib/actions/service';
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
	import { stopAllServices, restartAllServices } from '$lib/api';
	import Spinner from './Spinner.svelte';
	import StatusDot from './StatusDot.svelte';

	interface Props {
		selectedProject: string | null;
		onSelectProject: (name: string | null) => void;
		onInspect: (serviceName: string) => void;
	}

	interface Project {
		name: string;
		services: ServiceStatus[];
	}

	type StatusDotStatus =
		| 'running'
		| 'stopped'
		| 'building'
		| 'healthy'
		| 'starting'
		| 'unhealthy'
		| 'connected'
		| 'disconnected'
		| 'unknown';

	let { selectedProject, onSelectProject, onInspect }: Props = $props();

	async function handleStopAll() {
		if (!confirm('Are you sure you want to stop all services?')) return;
		try {
			await stopAllServices();
		} catch (e) {
			toasts.error(e instanceof Error ? e.message : String(e));
		}
	}

	async function handleRestartAll() {
		if (!confirm('Are you sure you want to restart all services?')) return;
		try {
			await restartAllServices();
		} catch (e) {
			toasts.error(e instanceof Error ? e.message : String(e));
		}
	}

	function isPending(serviceName: string): boolean {
		return $pendingActions.some((a) => a.serviceName === serviceName);
	}

	function handleServiceAction(
		e: Event,
		action: 'start' | 'stop' | 'restart',
		serviceName: string
	) {
		e.stopPropagation();
		if (action === 'start') startServiceWithFeedback(serviceName);
		if (action === 'stop') stopServiceWithFeedback(serviceName);
		if (action === 'restart') restartServiceWithFeedback(serviceName);
	}

	function getDisplayName(service: ServiceStatus) {
		return service.name.split(':').pop();
	}

	function getStatus(service: ServiceStatus): StatusDotStatus {
		if (service.status === 'building') return 'building';
		if (service.health_status === 'Healthy') return 'healthy';
		if (service.health_status === 'Starting') return 'starting';
		if (service.health_status === 'Unhealthy') return 'unhealthy';
		if (service.status === 'running') return 'running';
		if (service.status === 'stopped') return 'stopped';
		return 'unknown';
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
			{@const pending = isPending(service.name)}
			{@const status = getStatus(service)}
			<div
				class="nav-item sub-item sidebar-item"
				onclick={() => onInspect(service.name)}
				onkeydown={(e) => {
					if ((e.key === 'Enter' || e.key === ' ') && e.target === e.currentTarget) {
						e.preventDefault();
						onInspect(service.name);
					}
				}}
				role="button"
				tabindex="0"
			>
				<div class="service-info">
					<StatusDot {status} size="sm" />
					<span class="name">
						{getDisplayName(service)}
					</span>
				</div>

				<div class="sidebar-actions">
					{#if service.status === 'running'}
						<button
							title="Restart"
							disabled={pending}
							onclick={(e) => handleServiceAction(e, 'restart', service.name)}
						>
							{#if pending}
								<Spinner size={12} />
							{:else}
								<RotateCw size={12} />
							{/if}
						</button>
					{:else}
						<button
							title="Start"
							disabled={pending}
							onclick={(e) => handleServiceAction(e, 'start', service.name)}
						>
							{#if pending}
								<Spinner size={12} />
							{:else}
								<Play size={12} />
							{/if}
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
	button:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}

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
	.nav-item:focus-visible {
		outline: 2px solid #3b82f6;
		outline-offset: -2px;
		background: rgba(255, 255, 255, 0.05);
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
	.sidebar-item:focus-visible {
		outline: 2px solid #3b82f6; /* Blue-500 */
		outline-offset: 2px;
	}

	.service-info {
		display: flex;
		align-items: center;
		gap: 8px;
		overflow: hidden;
		min-width: 0; /* Allow truncation */
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
	.sidebar-actions button:focus-visible,
	.sidebar-actions a:focus-visible {
		outline: 2px solid #3b82f6;
		outline-offset: 2px;
	}

	.global-controls button:focus-visible {
		outline: 2px solid #3b82f6;
		outline-offset: 2px;
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
