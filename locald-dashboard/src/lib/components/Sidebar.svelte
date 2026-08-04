<script lang="ts">
	import {
		projectList,
		activeProjects,
		alwaysOnProjects,
		recentProjects
	} from '$lib/stores/projects';
	import { removeProject, type ProjectListEntry } from '$lib/api';
	import { toasts } from '$lib/stores/toasts';
	import { stopAllServices, restartAllServices } from '$lib/api';
	import { RotateCw, Square, Box, Activity, Pin, Clock, Circle } from 'lucide-svelte';
	import StatusDot from './StatusDot.svelte';

	interface Props {
		selectedProject: string | null;
		onSelectProject: (path: string | null) => void;
	}

	let { selectedProject, onSelectProject }: Props = $props();

	let contextMenu = $state<{ x: number; y: number; project: ProjectListEntry } | null>(null);

	async function handleStopAll() {
		if (!confirm('Are you sure you want to stop all managed services?')) return;
		try {
			await stopAllServices();
		} catch (e) {
			toasts.error(e instanceof Error ? e.message : String(e));
		}
	}

	async function handleRestartAll() {
		if (!confirm('Are you sure you want to restart all managed services?')) return;
		try {
			await restartAllServices();
		} catch (e) {
			toasts.error(e instanceof Error ? e.message : String(e));
		}
	}

	function handleContextMenu(e: MouseEvent, project: ProjectListEntry) {
		e.preventDefault();
		contextMenu = { x: e.clientX, y: e.clientY, project };
	}

	function closeContextMenu() {
		contextMenu = null;
	}

	async function handleRemove() {
		if (!contextMenu) return;
		const { project } = contextMenu;
		const name = project.project_name || project.project_path.split('/').pop() || 'this project';
		closeContextMenu();

		if (project.is_running) {
			if (!confirm(`"${name}" is running. Remove will stop all its services. Continue?`)) return;
		}

		try {
			await removeProject(project.project_path);
			await projectList.refresh();
			if (selectedProject === project.project_path) {
				onSelectProject(null);
			}
			toasts.success(`Removed ${name}`);
		} catch (e) {
			toasts.error(e instanceof Error ? e.message : String(e));
		}
	}

	function getProjectDisplayName(project: ProjectListEntry): string {
		return project.project_name || project.project_path.split('/').pop() || 'unknown';
	}

	function getProjectStatus(project: ProjectListEntry): 'running' | 'stopped' {
		return project.is_running ? 'running' : 'stopped';
	}

	function getAttachmentSummary(project: ProjectListEntry): string {
		const count = project.attachments.length;
		if (count === 0) return '';
		const editors = project.attachments.filter((a) => a.source.Editor).length;
		if (editors > 0) return `${editors} editor${editors > 1 ? 's' : ''}`;
		return `${count} attachment${count > 1 ? 's' : ''}`;
	}
</script>

<svelte:window onclick={closeContextMenu} />

<div class="sidebar">
	<div class="header">
		<div class="brand">
			<Box size={20} />
			<span>locald</span>
		</div>
		<div class="global-controls">
			<button title="Restart All Managed Services" onclick={handleRestartAll}>
				<RotateCw size={16} />
			</button>
			<button title="Stop All Managed Services" onclick={handleStopAll}>
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
			<span>Overview</span>
		</button>

		{#if $activeProjects.length > 0}
			<div class="section-divider"></div>
			<div class="section-header">
				<Circle size={10} fill="currentColor" color="#22c55e" />
				<span>Active</span>
			</div>
			{#each $activeProjects as project (project.project_path)}
				<button
					class="nav-item project-item"
					class:selected={selectedProject === project.project_path}
					onclick={() => onSelectProject(project.project_path)}
					oncontextmenu={(e) => handleContextMenu(e, project)}
				>
					<div class="project-info">
						<StatusDot status={getProjectStatus(project)} size="sm" />
						<span class="project-name">{getProjectDisplayName(project)}</span>
					</div>
					{#if getAttachmentSummary(project)}
						<span class="attachment-badge">{getAttachmentSummary(project)}</span>
					{/if}
				</button>
			{/each}
		{/if}

		{#if $alwaysOnProjects.length > 0}
			<div class="section-divider"></div>
			<div class="section-header">
				<Pin size={10} />
				<span>Always On</span>
			</div>
			{#each $alwaysOnProjects as project (project.project_path)}
				<button
					class="nav-item project-item"
					class:selected={selectedProject === project.project_path}
					onclick={() => onSelectProject(project.project_path)}
					oncontextmenu={(e) => handleContextMenu(e, project)}
				>
					<div class="project-info">
						<StatusDot status={getProjectStatus(project)} size="sm" />
						<span class="project-name">{getProjectDisplayName(project)}</span>
					</div>
				</button>
			{/each}
		{/if}

		{#if $recentProjects.length > 0}
			<div class="section-divider"></div>
			<div class="section-header">
				<Clock size={10} />
				<span>Recent</span>
			</div>
			{#each $recentProjects as project (project.project_path)}
				<button
					class="nav-item project-item"
					class:selected={selectedProject === project.project_path}
					onclick={() => onSelectProject(project.project_path)}
					oncontextmenu={(e) => handleContextMenu(e, project)}
				>
					<div class="project-info">
						<StatusDot status={getProjectStatus(project)} size="sm" />
						<span class="project-name">{getProjectDisplayName(project)}</span>
					</div>
				</button>
			{/each}
		{/if}
	</div>
</div>

{#if contextMenu}
	<div
		class="context-menu"
		style="left: {contextMenu.x}px; top: {contextMenu.y}px"
		role="menu"
		tabindex="-1"
		onclick={(e) => e.stopPropagation()}
		onkeydown={(e) => e.stopPropagation()}
	>
		<button class="context-menu-item danger" onclick={handleRemove} role="menuitem">
			Remove Project
		</button>
	</div>
{/if}

<style>
	button:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}

	.sidebar {
		width: 280px;
		background: #09090b;
		border-right: 1px solid #27272a;
		display: flex;
		flex-direction: column;
		height: 100%;
	}

	.header {
		padding: 16px;
		border-bottom: 1px solid #27272a;
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
		color: #e4e4e7;
	}

	.global-controls {
		display: flex;
		gap: 4px;
	}

	.global-controls button {
		background: transparent;
		border: none;
		color: #a1a1aa;
		cursor: pointer;
		padding: 4px;
		border-radius: 4px;
	}
	.global-controls button:hover {
		background: #27272a;
		color: #e4e4e7;
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
		color: #a1a1aa;
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
		background: rgba(255, 255, 255, 0.05);
		color: #e4e4e7;
	}
	.nav-item:focus-visible {
		outline: 2px solid #3b82f6;
		outline-offset: -2px;
		background: rgba(255, 255, 255, 0.05);
	}

	.nav-item.selected {
		background: #27272a;
		color: #e4e4e7;
		font-weight: 500;
	}

	.project-item {
		padding-left: 16px;
	}

	.project-info {
		display: flex;
		align-items: center;
		gap: 8px;
		overflow: hidden;
		min-width: 0;
	}

	.project-name {
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.attachment-badge {
		font-size: 0.7rem;
		color: #71717a;
		white-space: nowrap;
		flex-shrink: 0;
	}

	.section-divider {
		height: 1px;
		background: #27272a;
		margin: 12px 0 8px 0;
	}

	.section-header {
		display: flex;
		align-items: center;
		gap: 6px;
		padding: 0 12px;
		font-size: 0.7rem;
		text-transform: uppercase;
		color: #71717a;
		font-weight: 600;
		margin-bottom: 4px;
		letter-spacing: 0.05em;
	}

	.global-controls button:focus-visible {
		outline: 2px solid #3b82f6;
		outline-offset: 2px;
	}

	/* Context menu */
	.context-menu {
		position: fixed;
		z-index: 100;
		background: #18181b;
		border: 1px solid #3f3f46;
		border-radius: 6px;
		padding: 4px;
		min-width: 160px;
		box-shadow: 0 8px 24px rgba(0, 0, 0, 0.5);
	}

	.context-menu-item {
		display: block;
		width: 100%;
		padding: 8px 12px;
		background: transparent;
		border: none;
		color: #e4e4e7;
		cursor: pointer;
		border-radius: 4px;
		text-align: left;
		font-size: 0.85rem;
	}

	.context-menu-item:hover {
		background: rgba(255, 255, 255, 0.05);
	}

	.context-menu-item.danger {
		color: #ef4444;
	}

	.context-menu-item.danger:hover {
		background: rgba(239, 68, 68, 0.1);
	}
</style>
