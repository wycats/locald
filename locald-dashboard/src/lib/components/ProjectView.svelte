<script lang="ts">
	/* eslint-disable svelte/no-navigation-without-resolve */
	import { services } from '$lib/stores/services';
	import { projectList } from '$lib/stores/projects';
	import { pendingActions } from '$lib/stores/actions';
	import {
		startServiceWithFeedback,
		stopServiceWithFeedback,
		restartServiceWithFeedback
	} from '$lib/actions/service';
	import type { ProjectListEntry } from '$lib/api';
	import type { ServiceStatus } from '$lib/types';
	import {
		RotateCw,
		Square,
		Play,
		ExternalLink,
		Terminal as TerminalIcon,
		Folder,
		Users,
		Pin
	} from 'lucide-svelte';
	import Spinner from './Spinner.svelte';
	import Terminal from './Terminal.svelte';

	interface Props {
		projectPath: string;
		initialService?: string | null;
	}

	let { projectPath, initialService = null }: Props = $props();

	let localServiceOverride = $state<string | null>(null);
	let selectedService = $derived(localServiceOverride ?? initialService);

	function toggleServiceFilter(name: string) {
		localServiceOverride = selectedService === name ? null : name;
	}

	let project = $derived(
		$projectList.find((p: ProjectListEntry) => p.project_path === projectPath)
	);

	let projectServices = $derived($services.filter((s: ServiceStatus) => s.path === projectPath));

	let displayName = $derived(project?.project_name || projectPath.split('/').pop() || 'Unknown');

	let sectionLabel = $derived.by(() => {
		if (!project) return '';
		switch (project.section) {
			case 'Active':
				return 'Active';
			case 'AlwaysOn':
				return 'Always On';
			case 'Recent':
				return 'Recent';
		}
	});

	let editorCount = $derived(project?.attachments.filter((a) => a.source.Editor).length ?? 0);

	let isPinned = $derived(project?.attachments.some((a) => a.source.Pin !== undefined) ?? false);

	function isPending(serviceName: string): boolean {
		return $pendingActions.some((a) => a.serviceName === serviceName);
	}

	function getDisplayName(service: ServiceStatus): string {
		const parts = service.name.split(':');
		return parts.length > 1 ? parts[parts.length - 1] : service.name;
	}

	function getServiceType(service: ServiceStatus): string {
		if (service.service_type) {
			switch (service.service_type) {
				case 'postgres':
					return 'db';
				case 'container':
					if (service.name.includes('redis') || service.name.includes('cache')) return 'cache';
					return 'container';
				case 'worker':
					return 'worker';
				case 'site':
					return 'site';
				case 'exec':
				default:
					return service.port ? 'web' : 'worker';
			}
		}
		if (service.name.includes('db') || service.name.includes('postgres')) return 'db';
		if (service.name.includes('redis') || service.name.includes('cache')) return 'cache';
		if (service.port) return 'web';
		return 'worker';
	}

	async function handleServiceAction(
		e: Event,
		action: 'start' | 'stop' | 'restart',
		serviceName: string
	) {
		e.stopPropagation();
		if (action === 'start') await startServiceWithFeedback(serviceName);
		if (action === 'stop') await stopServiceWithFeedback(serviceName);
		if (action === 'restart') await restartServiceWithFeedback(serviceName);
	}

	function splitDomain(domain: string): { service: string; project: string; tld: string } {
		// Domain format: service.project.localhost
		const parts = domain.split('.');
		if (parts.length >= 3) {
			return {
				service: parts.slice(0, -2).join('.'),
				project: parts[parts.length - 2],
				tld: parts[parts.length - 1]
			};
		}
		return { service: domain, project: '', tld: '' };
	}
</script>

<div class="project-view">
	<div class="project-header">
		<div class="project-title">
			<h2 class="project-name-header">{displayName}</h2>
			<div class="project-meta">
				<span
					class="section-badge"
					class:active={project?.section === 'Active'}
					class:pinned={project?.section === 'AlwaysOn'}
				>
					{sectionLabel}
				</span>
				{#if editorCount > 0}
					<span class="meta-item">
						<Users size={12} />
						{editorCount} editor{editorCount > 1 ? 's' : ''}
					</span>
				{/if}
				{#if isPinned}
					<span class="meta-item pinned">
						<Pin size={12} />
						Pinned
					</span>
				{/if}
			</div>
		</div>
		<div class="project-path">
			<Folder size={12} />
			<span>{projectPath}</span>
		</div>
	</div>

	{#if projectServices.length === 0}
		<div class="empty-state">
			<p>No services found for this project.</p>
			<p class="hint">
				Services appear here when the project is started with <code>locald up</code>.
			</p>
		</div>
	{:else}
		<div class="service-list">
			{#each projectServices as service (service.name)}
				{@const pending = isPending(service.name)}
				{@const type = getServiceType(service)}
				<div
					class="service-row"
					class:disabled={service.status === 'stopped'}
					class:active={selectedService === service.name}
					onclick={() => toggleServiceFilter(service.name)}
					onkeydown={(e) => {
						if (e.key === 'Enter' || e.key === ' ') {
							e.preventDefault();
							toggleServiceFilter(service.name);
						}
					}}
					role="button"
					tabindex="0"
				>
					<div class="row-content">
						<div class="status-dot {service.status}"></div>
						<span class="service-name">{getDisplayName(service)}</span>

						{#if service.url && service.status === 'running'}
							<a
								href={service.url}
								target="_blank"
								class="type-chip {type} interactive"
								title="Open {service.url}"
								onclick={(e) => e.stopPropagation()}
							>
								{type}
								<ExternalLink size={9} />
							</a>
						{:else}
							<span class="type-chip {type}">{type}</span>
						{/if}

						{#if service.domain && service.status === 'running'}
							{@const parts = splitDomain(service.domain)}
							<a
								href={service.url}
								target="_blank"
								class="domain-link"
								onclick={(e) => e.stopPropagation()}
							>
								<span class="domain-service">{parts.service}</span><span class="domain-project"
									>.{parts.project}</span
								><span class="domain-tld">.{parts.tld}</span>
								<ExternalLink size={9} />
							</a>
						{/if}
					</div>

					<div class="row-toolbar">
						<div class="toolbar-bg"></div>
						<div class="toolbar-actions">
							{#if service.status === 'running'}
								<button
									class="control-btn"
									title="Restart"
									disabled={pending}
									onclick={(e) => handleServiceAction(e, 'restart', service.name)}
								>
									{#if pending}
										<Spinner size={14} />
									{:else}
										<RotateCw size={14} />
									{/if}
								</button>
								<button
									class="control-btn"
									title="Stop"
									disabled={pending}
									onclick={(e) => handleServiceAction(e, 'stop', service.name)}
								>
									<Square size={14} />
								</button>
							{:else}
								<button
									class="control-btn"
									title="Start"
									disabled={pending}
									onclick={(e) => handleServiceAction(e, 'start', service.name)}
								>
									{#if pending}
										<Spinner size={14} />
									{:else}
										<Play size={14} />
									{/if}
								</button>
							{/if}
							<button
								class="control-btn"
								title="Filter logs"
								onclick={(e) => {
									e.stopPropagation();
									toggleServiceFilter(service.name);
								}}
							>
								<TerminalIcon size={14} />
							</button>
						</div>
					</div>
				</div>
			{/each}
		</div>

		<div class="log-area">
			<Terminal filter={selectedService} />
		</div>
	{/if}
</div>

<style>
	.project-view {
		display: flex;
		flex-direction: column;
		height: 100%;
		overflow: hidden;
	}

	.project-header {
		padding: 24px;
		border-bottom: 1px solid #27272a;
		background: #09090b;
	}

	.project-title {
		display: flex;
		align-items: center;
		gap: 12px;
		margin-bottom: 8px;
	}

	.project-title h2 {
		margin: 0;
		font-size: 1.25rem;
		font-weight: 600;
		color: #e4e4e7;
	}

	.project-name-header {
		color: #71717a;
	}

	.project-meta {
		display: flex;
		align-items: center;
		gap: 8px;
	}

	.section-badge {
		font-size: 0.7rem;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		padding: 2px 8px;
		border-radius: 4px;
		font-weight: 600;
		background: #27272a;
		color: #a1a1aa;
	}

	.section-badge.active {
		background: rgba(34, 197, 94, 0.15);
		color: #22c55e;
	}

	.section-badge.pinned {
		background: rgba(59, 130, 246, 0.15);
		color: #3b82f6;
	}

	.meta-item {
		display: flex;
		align-items: center;
		gap: 4px;
		font-size: 0.75rem;
		color: #71717a;
	}

	.meta-item.pinned {
		color: #3b82f6;
	}

	.project-path {
		display: flex;
		align-items: center;
		gap: 6px;
		font-size: 0.8rem;
		color: #52525b;
		font-family: var(--font-mono, monospace);
	}

	.empty-state {
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		padding: 48px 24px;
		color: #71717a;
		text-align: center;
	}

	.empty-state p {
		margin: 4px 0;
	}

	.empty-state .hint {
		font-size: 0.85rem;
		color: #52525b;
	}

	.empty-state code {
		background: #27272a;
		padding: 2px 6px;
		border-radius: 4px;
		font-family: var(--font-mono, monospace);
		font-size: 0.85rem;
		color: #a1a1aa;
	}

	.service-list {
		flex-shrink: 0;
		overflow-y: auto;
	}

	.log-area {
		flex: 1;
		min-height: 0;
		border-top: 1px solid #27272a;
	}

	/* --- Rack-style service rows --- */
	.service-row {
		--row-bg: #09090b;
		display: grid;
		grid-template-areas: 'stack';
		grid-template-columns: 100%;
		grid-template-rows: 100%;
		align-items: center;
		padding: 0 12px;
		border-bottom: 1px solid #27272a;
		cursor: pointer;
		transition: background 0.2s;
		height: 40px;
		position: relative;
		background: var(--row-bg);
	}
	.service-row.disabled {
		opacity: 0.5;
		--row-bg: #121214;
	}
	.service-row:hover {
		--row-bg: #18181b;
	}
	.service-row.active {
		--row-bg: #27272a;
		border-left: 2px solid #fff;
	}
	.service-row.active .service-name {
		color: #fff;
	}
	.service-row:focus-visible {
		outline: 2px solid #3b82f6;
		outline-offset: -2px;
		--row-bg: #18181b;
	}

	.row-content {
		grid-area: stack;
		display: flex;
		align-items: center;
		gap: 8px;
		width: 100%;
		min-width: 0;
		z-index: 1;
		padding-right: 80px;
	}

	.status-dot {
		width: 6px;
		height: 6px;
		border-radius: 50%;
		flex-shrink: 0;
		background: #a1a1aa;
		box-shadow: 0 0 0 1px rgba(113, 113, 122, 0.2);
	}
	.status-dot.running {
		background: #4ade80;
		box-shadow: 0 0 0 1px rgba(74, 222, 128, 0.2);
	}
	.status-dot.building {
		background: #c084fc;
		box-shadow: 0 0 0 1px rgba(192, 132, 252, 0.2);
		animation: pulse 1.5s infinite;
	}
	.status-dot.stopped {
		background: #52525b;
		box-shadow: none;
	}

	@keyframes pulse {
		0% {
			opacity: 1;
		}
		50% {
			opacity: 0.5;
		}
		100% {
			opacity: 1;
		}
	}

	.service-name {
		font-size: 13px;
		font-weight: 500;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		color: #71717a;
		transition: color 0.2s;
	}
	.service-row:hover .service-name {
		color: #fff;
	}

	.type-chip {
		font-size: 10px;
		font-weight: 600;
		text-transform: uppercase;
		color: #71717a;
		letter-spacing: 0.02em;
		line-height: 1;
		flex-shrink: 0;
		padding: 3px 6px;
		border-radius: 6px;
		border: 1px solid rgba(113, 113, 122, 0.2);
		background: rgba(113, 113, 122, 0.05);
	}
	.type-chip.db {
		color: #a78bfa;
		border-color: rgba(167, 139, 250, 0.2);
		background: rgba(167, 139, 250, 0.05);
	}
	.type-chip.web {
		color: #60a5fa;
		border-color: rgba(96, 165, 250, 0.2);
		background: rgba(96, 165, 250, 0.05);
	}
	.type-chip.site {
		color: #2dd4bf;
		border-color: rgba(45, 212, 191, 0.2);
		background: rgba(45, 212, 191, 0.05);
	}
	.type-chip.worker {
		color: #f472b6;
		border-color: rgba(244, 114, 182, 0.2);
		background: rgba(244, 114, 182, 0.05);
	}
	.type-chip.cache {
		color: #fbbf24;
		border-color: rgba(251, 191, 36, 0.2);
		background: rgba(251, 191, 36, 0.05);
	}
	.type-chip.interactive {
		cursor: pointer;
		display: flex;
		align-items: center;
		gap: 4px;
		text-decoration: none;
		transition: all 0.2s;
	}
	.type-chip.interactive:hover {
		color: #fff;
		border-color: #52525b;
		background: #27272a;
	}

	.port-label {
		font-size: 11px;
		color: #52525b;
		font-family: var(--font-mono, monospace);
		flex-shrink: 0;
	}

	.domain-link {
		display: flex;
		align-items: center;
		gap: 4px;
		font-size: 11px;
		color: #52525b;
		text-decoration: none;
		font-family: var(--font-mono, monospace);
		flex-shrink: 0;
		transition: color 0.2s;
	}
	.domain-link:hover {
		color: #a1a1aa;
	}
	.domain-link:hover .domain-service {
		color: #fff;
	}

	.domain-service {
		color: #e4e4e7;
	}
	.domain-project {
		color: #71717a;
	}
	.domain-tld {
		color: #3f3f46;
	}

	/* --- Toolbar overlay (Rack-style) --- */
	.row-toolbar {
		grid-area: stack;
		justify-self: end;
		z-index: 2;
		display: flex;
		align-items: center;
		height: 100%;
		position: relative;
		padding-left: 48px;
		opacity: 0;
		pointer-events: none;
		transition: opacity 0.1s;
	}
	.service-row:hover .row-toolbar {
		opacity: 1;
		pointer-events: auto;
	}

	.toolbar-bg {
		position: absolute;
		inset: 0;
		z-index: -1;
		background: linear-gradient(to left, var(--row-bg) 60%, transparent);
		pointer-events: none;
	}

	.toolbar-actions {
		display: flex;
		align-items: center;
		gap: 6px;
		height: 100%;
		padding-right: 4px;
	}

	.control-btn {
		background: none;
		border: none;
		color: #71717a;
		cursor: pointer;
		padding: 6px;
		border-radius: 6px;
		display: flex;
		align-items: center;
		justify-content: center;
		transition: all 0.2s;
	}
	.control-btn:hover {
		color: #e4e4e7;
		background: rgba(255, 255, 255, 0.05);
	}
	.control-btn:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}
	.control-btn:focus-visible {
		outline: 2px solid #3b82f6;
		outline-offset: 2px;
	}
</style>
