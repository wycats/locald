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
	import {
		pauseProjectWithFeedback,
		resumeProjectWithFeedback,
		setProjectAlwaysOnWithFeedback
	} from '$lib/actions/project';
	import {
		availabilityLabel,
		availabilityMessage,
		demandSummary,
		formatTransition,
		projectCanPause,
		projectCanResume,
		transitionLabel
	} from '$lib/availability';
	import type { ProjectListEntry } from '$lib/api';
	import { serviceIdentity, type ServiceStatus } from '$lib/types';
	import {
		publicationGuidance,
		managedServices,
		serviceDisplayAuthority,
		serviceLifecycleLabel,
		serviceLifecycleSummary
	} from '$lib/service-presentation';
	import {
		RotateCw,
		Square,
		Play,
		ExternalLink,
		Folder,
		Layers,
		X,
		Pause,
		Pin,
		PinOff
	} from 'lucide-svelte';
	import Spinner from './Spinner.svelte';
	import Terminal from './Terminal.svelte';

	interface Props {
		projectPath: string;
		monitored: string[];
		onToggleMonitor: (name: string | string[]) => void;
		onDeselectProject: () => void;
	}

	let {
		projectPath,
		monitored = [],
		// eslint-disable-next-line @typescript-eslint/no-unused-vars
		onToggleMonitor = (_name: string | string[]) => {},
		onDeselectProject = () => {}
	}: Props = $props();

	let project = $derived(
		$projectList.find((p: ProjectListEntry) => p.project_path === projectPath)
	);

	let projectServices = $derived($services.filter((s: ServiceStatus) => s.path === projectPath));
	let managedProjectServices = $derived(managedServices(projectServices));
	let projectAction = $state<'resume' | 'pause' | 'always-on' | null>(null);

	let displayName = $derived(project?.project_name || projectPath.split('/').pop() || 'Unknown');
	let availabilityState = $derived(availabilityLabel(project?.availability));
	let availabilityExplanation = $derived(availabilityMessage(project?.availability));
	let liveDemands = $derived(demandSummary(project?.availability));
	let nextTransition = $derived(formatTransition(project?.availability?.next_transition_at));

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

	let sectionSubtitle = $derived.by(() => {
		if (!project) return '';
		switch (project.section) {
			case 'Active':
				return 'Demanded or currently available';
			case 'AlwaysOn':
				return 'Kept available';
			case 'Recent':
				return 'Known project';
		}
	});

	let deckCount = $derived(
		managedProjectServices.filter((service: ServiceStatus) =>
			monitored.includes(serviceIdentity(service))
		).length
	);

	function isPending(service: ServiceStatus): boolean {
		return $pendingActions.some(
			(action) => action.serviceName === service.name && action.instanceId === service.instance_id
		);
	}

	function getDisplayName(service: ServiceStatus): string {
		const parts = service.name.split(':');
		return parts.length > 1 ? parts[parts.length - 1] : service.name;
	}

	function getServiceType(service: ServiceStatus): string {
		if (service.service_type) {
			switch (service.service_type) {
				case 'published':
					return 'published';
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
		service: ServiceStatus
	) {
		e.stopPropagation();
		if (action === 'start') await startServiceWithFeedback(service.name, service.instance_id);
		if (action === 'stop') await stopServiceWithFeedback(service.name, service.instance_id);
		if (action === 'restart') await restartServiceWithFeedback(service.name, service.instance_id);
	}

	async function handleProjectAction(action: 'resume' | 'pause' | 'always-on') {
		if (!project || projectAction) return;
		projectAction = action;
		try {
			if (action === 'resume') await resumeProjectWithFeedback(project);
			if (action === 'pause') await pauseProjectWithFeedback(project);
			if (action === 'always-on') {
				await setProjectAlwaysOnWithFeedback(project, !project.availability?.always_on);
			}
		} finally {
			projectAction = null;
		}
	}

	function displayUrl(service: ServiceStatus): string {
		return serviceDisplayAuthority(service);
	}
</script>

<div class="project-view">
	<div class="project-header">
		<div class="project-title">
			<h2 class="project-name-header">{displayName}</h2>
			<button class="project-close" title="Back to stream" onclick={onDeselectProject}>
				<X size={14} />
			</button>
			<div class="project-meta">
				<span
					class="section-badge"
					class:active={project?.section === 'Active'}
					class:always-on={project?.section === 'AlwaysOn'}
				>
					{sectionLabel}
				</span>
				{#if sectionSubtitle}
					<span class="meta-item section-subtitle">{sectionSubtitle}</span>
				{/if}
				{#if deckCount > 0}
					<span class="meta-item deck">
						<Layers size={12} />
						{deckCount} in Deck
					</span>
				{/if}
			</div>
		</div>
		<div class="project-path">
			<Folder size={12} />
			<span>{projectPath}</span>
		</div>
		{#if project}
			<div class="availability-panel" data-state={project.availability?.state ?? 'unknown'}>
				<div class="availability-copy">
					<div class="availability-heading">
						<span class="availability-state">{availabilityState}</span>
						{#if project.availability?.always_on}
							<span class="always-on-chip">Always On</span>
						{/if}
					</div>
					<p>{availabilityExplanation}</p>
					<div class="availability-details">
						{#if liveDemands}
							<span>Demand: {liveDemands}</span>
						{/if}
						{#if nextTransition}
							<span>{transitionLabel(project.availability)} {nextTransition}</span>
						{/if}
					</div>
				</div>
				<div class="availability-actions">
					{#if projectCanPause(project.availability)}
						<button
							class="project-action secondary"
							disabled={projectAction !== null}
							onclick={() => handleProjectAction('pause')}
						>
							{#if projectAction === 'pause'}
								<Spinner size={14} />
							{:else}
								<Pause size={14} />
							{/if}
							Pause
						</button>
					{:else if projectCanResume(project.availability)}
						<button
							class="project-action primary"
							disabled={projectAction !== null}
							onclick={() => handleProjectAction('resume')}
						>
							{#if projectAction === 'resume'}
								<Spinner size={14} />
							{:else}
								<Play size={14} />
							{/if}
							Resume
						</button>
					{:else if project.availability?.state === 'missing'}
						<span class="missing-guidance">Restore the worktree to resume this project.</span>
					{:else}
						<span class="missing-guidance">
							{project.availability
								? 'No lifecycle action is currently available.'
								: 'Project availability has not been reported.'}
						</span>
					{/if}
					{#if project.availability && project.availability.state !== 'missing'}
						<button
							class="project-action secondary"
							disabled={projectAction !== null}
							onclick={() => handleProjectAction('always-on')}
						>
							{#if projectAction === 'always-on'}
								<Spinner size={14} />
							{:else if project.availability?.always_on}
								<PinOff size={14} />
							{:else}
								<Pin size={14} />
							{/if}
							{project.availability?.always_on ? 'Use automatic lifecycle' : 'Keep Always On'}
						</button>
					{/if}
				</div>
			</div>
		{/if}
	</div>

	{#if projectServices.length === 0}
		<div class="empty-state">
			<p>No services found for this project.</p>
			<p class="hint">
				Services appear here when the project is started with <code>locald up</code>.
			</p>
		</div>
	{:else}
		<div class="service-summary" aria-label="Service lifecycle summary">
			<span>{projectServices.length} service{projectServices.length === 1 ? '' : 's'}</span>
			<span>{serviceLifecycleSummary(projectServices)}</span>
		</div>
		<div class="service-list">
			{#each projectServices as service (serviceIdentity(service))}
				{@const identity = serviceIdentity(service)}
				{@const pending = isPending(service)}
				{@const type = getServiceType(service)}
				{@const urlLabel = displayUrl(service)}
				{@const published = service.service_type === 'published'}
				{@const inDeck = !published && monitored.includes(identity)}
				<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
				<div
					class="service-row"
					class:disabled={!published && service.status === 'stopped'}
					class:published
					class:monitored={inDeck}
					onclick={() => {
						if (!published) onToggleMonitor(identity);
					}}
					onkeydown={(e) => {
						if (!published && (e.key === 'Enter' || e.key === ' ')) {
							e.preventDefault();
							onToggleMonitor(identity);
						}
					}}
					role={published ? undefined : 'button'}
					tabindex={published ? undefined : 0}
				>
					<div class="row-content">
						<div class="status-dot {service.status}"></div>
						<span class="service-name">{getDisplayName(service)}</span>
						<span class="status-chip {service.status}">{serviceLifecycleLabel(service)}</span>

						<span class="type-chip {type}">{type === 'published' ? 'App-managed' : type}</span>

						{#if inDeck}
							<span class="deck-chip">In Deck</span>
						{/if}

						{#if service.url && (service.status === 'running' || published)}
							<a
								href={service.url}
								target="_blank"
								class="primary-url"
								title="Open {service.url}"
								onclick={(e) => e.stopPropagation()}
							>
								<span>{urlLabel}</span>
								<ExternalLink size={11} />
							</a>
						{/if}

						{#if service.publication}
							<span
								class="publication-guidance"
								title={publicationGuidance(service.publication).next_step ??
									publicationGuidance(service.publication).explanation}
							>
								{publicationGuidance(service.publication).explanation}
							</span>
						{/if}
					</div>

					{#if !published}
						<div class="row-toolbar">
							<div class="toolbar-bg"></div>
							<div class="toolbar-actions">
								{#if service.status === 'running'}
									<button
										class="control-btn"
										title="Restart"
										disabled={pending}
										onclick={(e) => handleServiceAction(e, 'restart', service)}
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
										onclick={(e) => handleServiceAction(e, 'stop', service)}
									>
										<Square size={14} />
									</button>
								{:else}
									<button
										class="control-btn"
										title="Start"
										disabled={pending}
										onclick={(e) => handleServiceAction(e, 'start', service)}
									>
										{#if pending}
											<Spinner size={14} />
										{:else}
											<Play size={14} />
										{/if}
									</button>
								{/if}
							</div>
						</div>
					{/if}
				</div>
			{/each}
		</div>

		{#if managedProjectServices.length > 0}
			<div class="log-area">
				<Terminal filter={null} />
			</div>
		{/if}
	{/if}
</div>

<style>
	.project-view {
		display: flex;
		flex-direction: column;
		flex: 1;
		min-height: 0;
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

	.project-close {
		background: none;
		border: none;
		color: #52525b;
		cursor: pointer;
		padding: 4px;
		border-radius: 4px;
		display: flex;
		align-items: center;
		transition: color 0.15s;
	}
	.project-close:hover {
		color: #e4e4e7;
		background: rgba(255, 255, 255, 0.05);
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

	.section-badge.always-on {
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

	.meta-item.deck {
		color: #3b82f6;
	}
	.meta-item.section-subtitle {
		color: #52525b;
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

	.availability-panel {
		display: flex;
		flex-wrap: wrap;
		align-items: flex-start;
		justify-content: space-between;
		gap: 12px 20px;
		margin-top: 16px;
		padding: 14px 16px;
		border: 1px solid #27272a;
		border-radius: 10px;
		background: #111113;
	}
	.availability-panel[data-state='failed'],
	.availability-panel[data-state='degraded'] {
		border-color: rgba(239, 68, 68, 0.45);
	}
	.availability-panel[data-state='paused'],
	.availability-panel[data-state='stopped'],
	.availability-panel[data-state='cooling_down'] {
		border-color: rgba(245, 158, 11, 0.35);
	}
	.availability-copy {
		flex: 1 1 240px;
		min-width: 0;
	}
	.availability-heading,
	.availability-details,
	.availability-actions {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
	}
	.availability-state {
		font-size: 0.85rem;
		font-weight: 700;
		color: #e4e4e7;
	}
	.always-on-chip {
		padding: 2px 6px;
		border-radius: 999px;
		background: rgba(59, 130, 246, 0.15);
		color: #93c5fd;
		font-size: 0.65rem;
		font-weight: 700;
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}
	.availability-copy p {
		margin: 5px 0 0;
		color: #a1a1aa;
		font-size: 0.8rem;
		line-height: 1.4;
	}
	.availability-details {
		margin-top: 7px;
		color: #71717a;
		font-size: 0.7rem;
	}
	.availability-actions {
		justify-content: flex-start;
		flex-shrink: 0;
	}
	.project-action {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		border: 1px solid #3f3f46;
		border-radius: 7px;
		padding: 7px 10px;
		font: inherit;
		font-size: 0.75rem;
		font-weight: 600;
		cursor: pointer;
	}
	.project-action.primary {
		background: #2563eb;
		border-color: #2563eb;
		color: white;
	}
	.project-action.secondary {
		background: #18181b;
		color: #d4d4d8;
	}
	.project-action:hover:not(:disabled) {
		filter: brightness(1.12);
	}
	.project-action:disabled {
		opacity: 0.55;
		cursor: wait;
	}
	.missing-guidance {
		max-width: 15rem;
		color: #a1a1aa;
		font-size: 11px;
		line-height: 1.4;
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
		overflow-x: hidden;
	}

	.service-summary {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 8px 12px;
		border-bottom: 1px solid #27272a;
		font-size: 11px;
		color: #71717a;
		background: #09090b;
	}
	.service-summary span:first-child {
		color: #a1a1aa;
		font-weight: 600;
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
	.service-row.published {
		cursor: default;
	}
	.service-row.published .row-content {
		padding-right: 0;
	}
	.service-row:hover {
		--row-bg: #18181b;
	}
	.service-row.monitored {
		--row-bg: #27272a;
		border-left: 2px solid #3b82f6; /* Blue-500 */
	}
	.service-row.monitored .service-name {
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
	.status-dot.externally_managed {
		background: #60a5fa;
		box-shadow: 0 0 0 1px rgba(96, 165, 250, 0.25);
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
	.status-chip,
	.deck-chip {
		font-size: 10px;
		font-weight: 600;
		line-height: 1;
		flex-shrink: 0;
		padding: 3px 6px;
		border-radius: 999px;
		border: 1px solid rgba(113, 113, 122, 0.2);
		background: rgba(113, 113, 122, 0.05);
		color: #71717a;
	}
	.status-chip.running {
		color: #86efac;
		border-color: rgba(74, 222, 128, 0.22);
		background: rgba(74, 222, 128, 0.07);
	}
	.status-chip.building {
		color: #d8b4fe;
		border-color: rgba(192, 132, 252, 0.22);
		background: rgba(192, 132, 252, 0.07);
	}
	.status-chip.stopped {
		color: #71717a;
	}
	.status-chip.externally_managed,
	.type-chip.published {
		color: #93c5fd;
		border-color: rgba(96, 165, 250, 0.22);
		background: rgba(96, 165, 250, 0.07);
	}
	.publication-guidance {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-size: 11px;
		color: #a1a1aa;
	}
	.deck-chip {
		color: #93c5fd;
		border-color: rgba(96, 165, 250, 0.22);
		background: rgba(96, 165, 250, 0.07);
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

	.primary-url {
		display: flex;
		align-items: center;
		gap: 6px;
		min-width: 0;
		font-size: 12px;
		font-weight: 500;
		color: #cbd5e1;
		text-decoration: none;
		font-family: var(--font-mono, monospace);
		letter-spacing: -0.01em;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		padding: 3px 7px;
		border-radius: 6px;
		background: rgba(34, 211, 238, 0.07);
		border: 1px solid rgba(34, 211, 238, 0.18);
		transition:
			color 0.2s,
			background 0.2s,
			border-color 0.2s;
	}
	.primary-url:hover {
		color: #f8fafc;
		background: rgba(34, 211, 238, 0.12);
		border-color: rgba(34, 211, 238, 0.35);
	}
	.primary-url span {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
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
