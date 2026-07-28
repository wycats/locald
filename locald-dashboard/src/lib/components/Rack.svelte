<script lang="ts">
	/* eslint-disable svelte/no-navigation-without-resolve */
	import { projects, services, servicesError, servicesLoading } from '$lib/stores/services';
	import {
		projectList,
		activeProjects,
		alwaysOnProjects,
		recentProjects
	} from '$lib/stores/projects';
	import { removeProject, type ProjectListEntry } from '$lib/api';
	import { pendingActions } from '$lib/stores/actions';
	import { toasts } from '$lib/stores/toasts';
	import {
		startServiceWithFeedback,
		stopServiceWithFeedback,
		restartServiceWithFeedback,
		resetServiceWithFeedback
	} from '$lib/actions/service';
	import { pauseProjectWithFeedback, resumeProjectWithFeedback } from '$lib/actions/project';
	import {
		availabilityLabel,
		demandSummary,
		projectCanPause,
		projectCanResume
	} from '$lib/availability';
	import {
		Activity,
		Layers,
		Power,
		ChevronRight,
		ChevronDown,
		MoreHorizontal,
		Monitor,
		RefreshCw,
		RotateCcw,
		ExternalLink,
		AlertCircle,
		Clock
	} from 'lucide-svelte';
	import { serviceIdentity, type ServiceStatus } from '$lib/types';
	import Spinner from './Spinner.svelte';

	export let monitored: string[] = [];
	export let selectedProject: string | null = null;
	export let onSelectProject: (path: string | null) => void = () => {};
	export let onToggleMonitor: (name: string | string[]) => void = () => {};

	let collapsedGroups: string[] = [];
	let activeMenu: string | null = null;
	let keyboardFocus: string | null = null;
	let pendingProjects: string[] = [];
	let contextMenu: { x: number; y: number; project: ProjectListEntry } | null = null;

	type ProjectSection = ProjectListEntry['section'];
	let collapsedSections: ProjectSection[] = [];

	const SECTION_COPY: Record<ProjectSection, { label: string; subtitle: string }> = {
		Active: {
			label: 'Active',
			subtitle: 'Demanded or currently available'
		},
		AlwaysOn: {
			label: 'Always On',
			subtitle: 'Kept available'
		},
		Recent: {
			label: 'Recent',
			subtitle: 'Known projects'
		}
	};

	function toggleSectionCollapse(section: ProjectSection) {
		if (collapsedSections.includes(section)) {
			collapsedSections = collapsedSections.filter((s) => s !== section);
		} else {
			collapsedSections = [...collapsedSections, section];
		}
	}

	$: allServices = $projects.flatMap((p) => p.services);

	type SectionHeader = { kind: 'section'; section: ProjectSection; count: number };
	type ProjectGroup = {
		kind: 'project';
		name: string;
		path: string | null;
		entry: ProjectListEntry | null;
		services: ServiceStatus[];
		section: ProjectSection | null;
	};
	type RackEntry = SectionHeader | ProjectGroup;

	// Build an ordered list: section headers interleaved with project groups
	$: rackEntries = buildRackEntries(
		$activeProjects,
		$alwaysOnProjects,
		$recentProjects,
		$projects,
		collapsedSections
	);

	function buildRackEntries(
		active: ProjectListEntry[],
		alwaysOn: ProjectListEntry[],
		recent: ProjectListEntry[],
		serviceProjects: { name: string; path: string | null; services: ServiceStatus[] }[],
		collapsed: ProjectSection[]
	): RackEntry[] {
		const entries: RackEntry[] = [];
		// eslint-disable-next-line svelte/prefer-svelte-reactivity
		const claimed = new Set<string>();

		function addSection(section: ProjectSection, list: ProjectListEntry[]) {
			if (list.length === 0) return;
			entries.push({ kind: 'section', section, count: list.length });
			// Always claim names so collapsed projects don't leak into the unattached list
			for (const entry of list) {
				const entryName = entry.project_name || entry.project_path.split('/').pop() || '';
				claimed.add(entryName);
			}
			if (collapsed.includes(section)) return;
			for (const entry of list) {
				const entryName = entry.project_name || entry.project_path.split('/').pop() || '';
				const group = serviceProjects.find(
					(project) =>
						project.path === entry.project_path ||
						(project.path === null && project.name === entryName)
				);
				entries.push({
					kind: 'project',
					name: entryName,
					path: entry.project_path,
					entry,
					services: group?.services ?? [],
					section
				});
			}
		}

		addSection('Active', active);
		addSection('AlwaysOn', alwaysOn);
		addSection('Recent', recent);

		// Projects with services but no attachment entry
		for (const p of serviceProjects) {
			if (!claimed.has(p.name)) {
				entries.push({
					kind: 'project',
					name: p.name,
					path: null,
					entry: null,
					services: p.services,
					section: null
				});
			}
		}

		return entries;
	}

	function rackEntryKey(entry: RackEntry): string {
		if (entry.kind === 'section') return `section:${entry.section}`;
		return `project:${entry.path ?? entry.name}`;
	}

	function handleProjectContextMenu(e: MouseEvent, project: ProjectListEntry) {
		e.preventDefault();
		e.stopPropagation();
		contextMenu = { x: e.clientX, y: e.clientY, project };
	}

	function closeContextMenu() {
		contextMenu = null;
	}

	async function handleRemoveProject() {
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

	function handleKeydown(event: KeyboardEvent) {
		if (event.target instanceof HTMLInputElement || event.target instanceof HTMLTextAreaElement)
			return;

		if (event.key === 'j' || event.key === 'ArrowDown') {
			moveFocus(1);
		} else if (event.key === 'k' || event.key === 'ArrowUp') {
			moveFocus(-1);
		} else if (event.key === 'Enter' || event.key === ' ') {
			event.preventDefault();
			if (keyboardFocus) toggleMonitor(keyboardFocus);
		} else if (event.key === 'Escape') {
			activeMenu = null;
		}
	}

	function moveFocus(direction: number) {
		if (allServices.length === 0) return;

		let currentIndex = -1;
		if (keyboardFocus) {
			currentIndex = allServices.findIndex((service) => serviceIdentity(service) === keyboardFocus);
		}

		let nextIndex = currentIndex + direction;

		// Wrap around or clamp? Let's clamp.
		if (nextIndex < 0) nextIndex = 0;
		if (nextIndex >= allServices.length) nextIndex = allServices.length - 1;

		keyboardFocus = serviceIdentity(allServices[nextIndex]);

		// Ensure visible
		ensureVisible(keyboardFocus);
	}

	function ensureVisible(name: string | null) {
		if (!name) return;
		// Simple implementation: find element and scrollIntoView
		// We need a way to ref the elements.
		// For now, let's skip complex scrolling logic or use document.getElementById if we add IDs.
		setTimeout(() => {
			const el = document.getElementById(`service-${name}`);
			if (el) el.scrollIntoView({ block: 'nearest' });
		}, 0);
	}

	function toggleMonitor(name: string, event?: Event) {
		if (event) event.stopPropagation();
		onToggleMonitor(name);
	}

	function toggleMonitorGroup(groupServices: ServiceStatus[]) {
		const serviceNames = groupServices.map(serviceIdentity);
		const allMonitored = serviceNames.every((name) => monitored.includes(name));
		const next = allMonitored
			? monitored.filter((n) => !serviceNames.includes(n))
			: [...new Set([...monitored, ...serviceNames])];
		onToggleMonitor(next);
	}

	function toggleMenu(name: string, event: Event) {
		event.stopPropagation();
		activeMenu = activeMenu === name ? null : name;
	}

	function closeMenu() {
		activeMenu = null;
	}

	function isPending(service: ServiceStatus): boolean {
		return $pendingActions.some(
			(action) => action.serviceName === service.name && action.instanceId === service.instance_id
		);
	}

	async function toggleGroup(groupServices: ServiceStatus[]) {
		const allStopped = groupServices.every((s) => s.status === 'stopped');
		await Promise.all(
			groupServices.map((s) => {
				if (allStopped) return startServiceWithFeedback(s.name, s.instance_id);
				return stopServiceWithFeedback(s.name, s.instance_id);
			})
		);
	}

	async function toggleProject(project: ProjectGroup) {
		if (!project.entry) {
			await toggleGroup(project.services);
			return;
		}
		const path = project.entry.project_path;
		if (pendingProjects.includes(path)) return;
		if (
			!projectCanPause(project.entry.availability) &&
			!projectCanResume(project.entry.availability)
		) {
			return;
		}
		pendingProjects = [...pendingProjects, path];
		try {
			if (projectCanPause(project.entry.availability)) {
				await pauseProjectWithFeedback(project.entry);
			} else {
				await resumeProjectWithFeedback(project.entry);
			}
		} finally {
			pendingProjects = pendingProjects.filter((candidate) => candidate !== path);
		}
	}

	function projectActionTitle(project: ProjectListEntry): string {
		if (projectCanPause(project.availability)) return 'Pause project';
		if (projectCanResume(project.availability)) return 'Resume project';
		if (project.availability?.state === 'missing') {
			return 'Restore the worktree to resume this project';
		}
		if (!project.availability) return 'Project availability has not been reported';
		return 'No lifecycle action is currently available';
	}

	function toggleGroupCollapse(group: string) {
		if (collapsedGroups.includes(group)) {
			collapsedGroups = collapsedGroups.filter((g) => g !== group);
		} else {
			collapsedGroups = [...collapsedGroups, group];
		}
	}

	function toggleSystemMonitor() {
		onToggleMonitor('locald');
	}

	function getServiceType(service: ServiceStatus): string {
		// Use the actual service_type from the API if available
		if (service.service_type) {
			switch (service.service_type) {
				case 'postgres':
					return 'db';
				case 'container':
					// Check if it's a cache-like container (redis, memcached, etc.)
					if (service.name.includes('redis') || service.name.includes('cache')) {
						return 'cache';
					}
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
		// Fallback heuristics for older API responses
		if (service.name.includes('db') || service.name.includes('postgres')) return 'db';
		if (service.name.includes('redis') || service.name.includes('cache')) return 'cache';
		if (service.port) return 'web';
		return 'worker';
	}

	function getDisplayName(serviceName: string, projectName: string): string {
		if (serviceName === projectName) return 'main';
		if (serviceName.startsWith(projectName)) {
			const trimmed = serviceName.slice(projectName.length);
			// Remove separator if present
			if ([':', '-', '_'].includes(trimmed[0])) {
				return trimmed.slice(1);
			}
			return trimmed;
		}
		return serviceName;
	}

	function displayUrl(service: ServiceStatus): string {
		if (service.domain) return service.domain;
		if (!service.url) return '';
		try {
			const parsed = new URL(service.url);
			return parsed.hostname.endsWith('.localhost') ? parsed.hostname : parsed.host;
		} catch {
			return service.url.replace(/^https?:\/\//, '');
		}
	}

	function projectCountLabel(count: number): string {
		return `${count} project${count === 1 ? '' : 's'}`;
	}

	function lifecycleSummary(services: ServiceStatus[]): string {
		if (services.length === 0) return 'No services';

		const running = services.filter((s) => s.status === 'running').length;
		const building = services.filter((s) => s.status === 'building').length;
		const stopped = services.length - running - building;
		const parts = [`${running} running`];

		if (building > 0) parts.push(`${building} building`);
		if (stopped > 0) parts.push(`${stopped} stopped`);

		return parts.join(' · ');
	}

	function lifecycleLabel(status: ServiceStatus['status']): string {
		return status[0].toUpperCase() + status.slice(1);
	}

	function deckSummary(services: ServiceStatus[]): string | null {
		const count = services.filter((service) => monitored.includes(serviceIdentity(service))).length;
		if (count === 0) return null;
		return `${count} in Deck`;
	}
</script>

<svelte:window on:keydown={handleKeydown} />

<!-- a11y: Click-to-close has keyboard alternative via Escape key in handleKeydown -->
<!-- svelte-ignore a11y-click-events-have-key-events -->
<!-- svelte-ignore a11y-no-noninteractive-element-interactions -->
<div
	class="rack"
	on:click={() => {
		closeMenu();
		closeContextMenu();
	}}
	role="application"
>
	<div class="rack-header">
		<div class="logo">locald</div>
	</div>

	<div class="rack-list">
		{#if $servicesLoading && $projects.length === 0}
			<div class="rack-state">
				<Spinner size={24} />
				<span class="state-title">Loading services...</span>
			</div>
		{:else if $servicesError}
			<div class="rack-state error">
				<AlertCircle size={24} />
				<span class="state-title">Failed to load services</span>
				<span class="state-message">{$servicesError}</span>
				<button class="retry-btn" on:click={() => services.refresh()}>
					<RefreshCw size={14} />
					Retry
				</button>
			</div>
		{:else if $projects.length === 0 && $projectList.length === 0}
			<div class="rack-state empty">
				<Layers size={24} />
				<span class="state-title">No services found</span>
				<span class="state-message">
					Run <code>locald up</code> in a project directory to start services.
				</span>
			</div>
		{:else}
			{#each rackEntries as rackEntry (rackEntryKey(rackEntry))}
				{#if rackEntry.kind === 'section'}
					{@const sectionCollapsed = collapsedSections.includes(rackEntry.section)}
					{@const sectionCopy = SECTION_COPY[rackEntry.section]}
					<button
						class="rack-section-header"
						on:click={() => toggleSectionCollapse(rackEntry.section)}
					>
						<div class="section-title-row">
							{#if sectionCollapsed}
								<ChevronRight size={10} />
							{:else}
								<ChevronDown size={10} />
							{/if}
							{#if rackEntry.section === 'Active'}
								<div class="section-dot active"></div>
							{:else if rackEntry.section === 'AlwaysOn'}
								<Layers size={10} />
							{:else}
								<Clock size={10} />
							{/if}
							<span>{sectionCopy.label}</span>
							<span class="section-count">{projectCountLabel(rackEntry.count)}</span>
						</div>
						<span class="section-subtitle">{sectionCopy.subtitle}</span>
					</button>
				{:else}
					{@const project = rackEntry}
					{@const isCollapsed = collapsedGroups.includes(project.name)}
					{@const isAllStopped =
						project.services.length === 0 || project.services.every((s) => s.status === 'stopped')}
					{@const projectLifecycle = lifecycleSummary(project.services)}
					{@const projectDeck = deckSummary(project.services)}
					{@const projectDemands = demandSummary(project.entry?.availability)}
					{@const projectAvailability = project.entry?.availability
						? availabilityLabel(project.entry.availability)
						: null}
					{@const projectPending =
						project.entry != null && pendingProjects.includes(project.entry.project_path)}
					<!-- svelte-ignore a11y-no-static-element-interactions -->
					<div
						class="rack-group-header"
						class:disabled={project.entry?.availability
							? !project.entry.availability.desired
							: isAllStopped}
						class:selected={project.path != null && selectedProject === project.path}
						on:contextmenu={(e) => project.entry && handleProjectContextMenu(e, project.entry)}
					>
						<div class="group-main">
							<button
								class="group-title"
								type="button"
								on:click={() =>
									project.path
										? onSelectProject(selectedProject === project.path ? null : project.path)
										: toggleGroupCollapse(project.name)}
							>
								<span>{project.name}</span>
							</button>
							<div class="group-meta" aria-label="Project state">
								<span>{projectAvailability ?? projectLifecycle}</span>
								{#if projectAvailability}
									<span class="group-cue">{projectLifecycle}</span>
								{/if}
								{#if projectDeck}
									<span class="group-cue deck">{projectDeck}</span>
								{/if}
								{#if projectDemands}
									<span class="group-cue">{projectDemands}</span>
								{/if}
							</div>
						</div>
						{#if project.entry || project.services.length > 0}
							{@const serviceNames = project.services.map(serviceIdentity)}
							{@const allMonitored =
								serviceNames.length > 0 && serviceNames.every((n) => monitored.includes(n))}
							{@const someMonitored = serviceNames.some((n) => monitored.includes(n))}
							<div class="group-actions">
								{#if project.services.length > 0}
									<button
										class="group-btn monitor-group-btn"
										class:active={allMonitored}
										class:partial={someMonitored && !allMonitored}
										on:click|stopPropagation={() => toggleMonitorGroup(project.services)}
										title={allMonitored ? 'Remove all from Deck' : 'Add all to Deck'}
									>
										<Layers size={12} />
									</button>
								{/if}
								<button
									class="group-btn"
									disabled={projectPending ||
										(project.entry
											? !projectCanPause(project.entry.availability) &&
												!projectCanResume(project.entry.availability)
											: false)}
									on:click|stopPropagation={() => toggleProject(project)}
									title={project.entry
										? projectActionTitle(project.entry)
										: isAllStopped
											? 'Start group'
											: 'Stop group'}
								>
									{#if projectPending}
										<Spinner size={12} />
									{:else}
										<Power
											size={12}
											color={project.entry
												? projectCanPause(project.entry.availability)
													? '#ef4444'
													: '#52525b'
												: isAllStopped
													? '#52525b'
													: '#ef4444'}
										/>
									{/if}
								</button>
							</div>
						{/if}
					</div>

					{#if !isCollapsed && project.services.length > 0}
						{#each project.services as service (serviceIdentity(service))}
							{@const identity = serviceIdentity(service)}
							{@const type = getServiceType(service)}
							{@const displayName = getDisplayName(service.name, project.name)}
							{@const urlLabel = displayUrl(service)}
							{@const inDeck = monitored.includes(identity)}

							<div
								id="service-{identity}"
								class="rack-item"
								class:monitored={inDeck}
								class:keyboard-focused={keyboardFocus === identity}
								class:disabled={service.status === 'stopped'}
								on:click={() => toggleMonitor(identity)}
								on:keydown={(e) => {
									if (e.key === 'Enter' || e.key === ' ') {
										e.preventDefault();
										toggleMonitor(identity);
									}
								}}
								role="button"
								tabindex="0"
							>
								<!-- Layer 1: Content (Left Group) -->
								<div class="item-content">
									<div class="status-dot {service.status}"></div>
									<span class="service-name" title={service.name}>{displayName}</span>
									<span class="status-chip {service.status}">{lifecycleLabel(service.status)}</span>
									<span class="type-chip {type}">{type}</span>
									{#if inDeck}
										<span class="deck-chip">In Deck</span>
									{/if}

									{#if service.url && service.status === 'running'}
										<a
											href={service.url}
											target="_blank"
											class="service-url"
											title="Open {service.url}"
											on:click={(e) => e.stopPropagation()}
										>
											<span>{urlLabel}</span>
											<ExternalLink size={10} />
										</a>
									{/if}
								</div>

								<!-- Layer 2: Toolbar Overlay -->
								<div class="item-toolbar">
									<div class="toolbar-bg"></div>
									<div class="toolbar-actions">
										<button
											class="control-btn monitor-btn"
											class:active={inDeck}
											title={inDeck ? 'Remove from Deck' : 'Add to Deck'}
											on:click={(e) => toggleMonitor(identity, e)}
										>
											<Monitor size={14} />
										</button>
										{#if service.status === 'running'}
											<button
												class="control-btn"
												title="Restart"
												disabled={isPending(service)}
												on:click|stopPropagation={() =>
													restartServiceWithFeedback(service.name, service.instance_id)}
											>
												{#if isPending(service)}
													<Spinner size={14} />
												{:else}
													<RefreshCw size={14} />
												{/if}
											</button>
											<div class="menu-wrapper">
												<button
													class="control-btn"
													on:click={(e) => toggleMenu(identity, e)}
													title="More"
												>
													<MoreHorizontal size={14} />
												</button>
												{#if activeMenu === identity}
													<!-- Menu container only stops event propagation -->
													<div
														class="menu-dropdown"
														on:click={(e) => e.stopPropagation()}
														role="menu"
														tabindex="-1"
													>
														<div class="menu-item info">
															<span>PID: {service.pid || '-'}</span>
															<span>Port: {service.port || '-'}</span>
														</div>
														<div class="menu-separator"></div>
														<button
															class="menu-action danger"
															disabled={isPending(service)}
															on:click={() =>
																resetServiceWithFeedback(service.name, service.instance_id)}
														>
															{#if isPending(service)}
																<Spinner size={12} />
															{:else}
																<RotateCcw size={12} />
															{/if}
															Reset
														</button>
														<button
															class="menu-action danger"
															disabled={isPending(service)}
															on:click={() =>
																stopServiceWithFeedback(service.name, service.instance_id)}
														>
															{#if isPending(service)}
																<Spinner size={12} />
															{:else}
																<Power size={12} />
															{/if}
															Stop
														</button>
													</div>
												{/if}
											</div>
										{:else}
											<button
												class="control-btn power-btn"
												disabled={isPending(service)}
												on:click|stopPropagation={() =>
													startServiceWithFeedback(service.name, service.instance_id)}
												title="Start"
											>
												{#if isPending(service)}
													<Spinner size={14} />
												{:else}
													<Power size={14} />
												{/if}
											</button>
										{/if}
									</div>
								</div>
							</div>
						{/each}
					{/if}
				{/if}
			{/each}
		{/if}
	</div>

	<div
		class="rack-footer"
		class:active={monitored.includes('locald')}
		on:click={toggleSystemMonitor}
		role="button"
		tabindex="0"
		on:keydown={(e) => e.key === 'Enter' && toggleSystemMonitor()}
	>
		<div class="status-summary">
			<Activity size={16} />
			<span>System Normal</span>
		</div>
	</div>

	{#if contextMenu}
		<!-- svelte-ignore a11y-no-static-element-interactions -->
		<!-- svelte-ignore a11y-click-events-have-key-events -->
		<div
			class="rack-context-menu"
			style="top: {contextMenu.y}px; left: {contextMenu.x}px;"
			on:click|stopPropagation
		>
			<button on:click={handleRemoveProject}>Remove project</button>
		</div>
	{/if}
</div>

<style>
	.rack {
		background: #09090b; /* Zinc-950 */
		border-right: 1px solid #27272a;
		display: flex;
		flex-direction: column;
		height: 100%;
		min-height: 0;
	}

	@media (max-width: 640px) {
		.rack {
			max-height: 50vh;
			border-right: none;
			border-bottom: 1px solid #27272a;
		}
	}

	.rack-header {
		padding: 16px;
		border-bottom: 1px solid #27272a;
		font-weight: bold;
		color: #e4e4e7; /* Zinc-200 */
	}

	.rack-list {
		flex: 1;
		overflow-y: auto;
		overflow-x: hidden;
		min-height: 0;
	}

	.rack-state {
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 8px;
		padding: 32px 24px;
		color: #a1a1aa;
		text-align: center;
	}

	.rack-state.error {
		color: #fca5a5;
	}

	.rack-state.empty {
		color: #a1a1aa;
	}

	.state-title {
		font-size: 13px;
		font-weight: 600;
		color: #e4e4e7;
	}

	.rack-state.error .state-title {
		color: #fecaca;
	}

	.state-message {
		font-size: 12px;
		color: #a1a1aa;
		max-width: 240px;
	}

	.rack-state.error .state-message {
		color: #fca5a5;
	}

	.retry-btn {
		margin-top: 8px;
		background: #1f2937;
		border: 1px solid #374151;
		color: #e5e7eb;
		border-radius: 6px;
		padding: 6px 10px;
		font-size: 12px;
		cursor: pointer;
		display: inline-flex;
		align-items: center;
		gap: 6px;
		transition:
			background 0.2s,
			border-color 0.2s;
	}
	.retry-btn:hover {
		background: #111827;
		border-color: #4b5563;
	}

	.rack-list::-webkit-scrollbar {
		width: 8px;
	}
	.rack-list::-webkit-scrollbar-track {
		background: transparent;
	}
	.rack-list::-webkit-scrollbar-thumb {
		background: #3f3f46;
		border-radius: 4px;
		border: 2px solid #18181b; /* Padding around thumb */
	}
	.rack-list::-webkit-scrollbar-thumb:hover {
		background: #52525b;
	}

	button:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}

	.rack-group-header {
		padding: 8px 16px 8px 20px;
		color: #a1a1aa;
		margin-top: 2px;
		display: flex;
		justify-content: space-between;
		align-items: center;
		gap: 8px;
		cursor: pointer;
	}
	.rack-group-header.disabled {
		opacity: 0.5;
	}
	.group-main {
		display: flex;
		flex-direction: column;
		gap: 3px;
		min-width: 0;
	}

	.group-title {
		display: flex;
		align-items: center;
		gap: 6px;
		background: none;
		border: none;
		color: inherit;
		font: inherit;
		cursor: pointer;
		padding: 0;
		font-size: 11px;
		font-weight: 700;
		letter-spacing: 0.05em;
		text-transform: uppercase;
		min-width: 0;
	}
	.group-title span {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.group-title:focus-visible {
		outline: 2px solid #3b82f6;
		outline-offset: 2px;
		border-radius: 4px;
	}
	.group-meta {
		display: flex;
		align-items: center;
		gap: 5px;
		min-width: 0;
		font-size: 10px;
		font-weight: 500;
		letter-spacing: 0;
		text-transform: none;
		color: #52525b;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.group-cue {
		color: #71717a;
	}
	.group-cue.deck {
		color: #93c5fd;
	}

	.group-actions {
		display: flex;
		gap: 4px;
		opacity: 0;
		transition: opacity 0.2s;
	}
	.rack-group-header:hover .group-actions {
		opacity: 1;
	}

	.group-btn {
		background: none;
		border: none;
		color: #71717a;
		cursor: pointer;
		padding: 2px;
	}
	.group-btn:hover {
		color: #fff;
	}
	.group-btn:focus-visible {
		outline: 2px solid #3b82f6;
		outline-offset: 2px;
		color: #fff;
	}
	.monitor-group-btn.active {
		color: #3b82f6; /* Blue-500 */
	}
	.monitor-group-btn.partial {
		color: #60a5fa; /* Blue-400, lighter for partial */
		opacity: 0.7;
	}

	/* --- Rack Item Layout --- */
	.rack-item {
		--row-bg: #09090b; /* Default: Zinc-950 */

		display: grid;
		grid-template-areas: 'stack';
		grid-template-columns: 100%;
		grid-template-rows: 100%;
		align-items: center;
		padding: 0 12px 0 28px;
		border-bottom: 1px solid #27272a;
		cursor: pointer;
		transition: background 0.2s;
		height: 36px; /* Tighter height */
		position: relative;
		background: var(--row-bg);
	}
	.rack-item.disabled {
		opacity: 0.5;
		--row-bg: #121214;
	}

	.rack-item:hover {
		--row-bg: #18181b; /* Zinc-900 (Approx match for 5% white overlay) */
	}
	.rack-item:focus-visible {
		outline: 2px solid #3b82f6;
		outline-offset: -2px;
		--row-bg: #18181b;
	}

	.rack-item.monitored {
		--row-bg: #27272a; /* Zinc-800 */
		border-left: 2px solid #fff;
	}

	.rack-item.keyboard-focused {
		--row-bg: #27272a;
	}

	/* --- Layer 1: Content --- */
	.item-content {
		grid-area: stack;
		display: flex;
		align-items: center;
		gap: 6px;
		width: 100%;
		min-width: 0;
		z-index: 1;
		padding-right: 72px; /* avoid toolbar overlap */
	}

	.status-dot {
		width: 6px;
		height: 6px;
		border-radius: 50%;
		flex-shrink: 0;
		background: #a1a1aa; /* Zinc-400 */
		box-shadow: 0 0 0 1px rgba(113, 113, 122, 0.2);
	}
	.status-dot.running {
		background: #4ade80; /* Green-400 */
		box-shadow: 0 0 0 1px rgba(74, 222, 128, 0.2);
	}
	.status-dot.building {
		background: #c084fc; /* Purple-400 */
		box-shadow: 0 0 0 1px rgba(192, 132, 252, 0.2);
		animation: pulse 1.5s infinite;
	}
	.status-dot.stopped {
		background: #52525b; /* Zinc-600 */
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
		transition: color 0.2s;
		/* State B: Inactive (Default) - Darker for contrast */
		color: #71717a; /* Zinc-500 */
	}

	/* Monitored & Hover -> White */
	.rack-item.monitored .service-name,
	.rack-item:hover .service-name {
		color: #ffffff;
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
		border-radius: 6px; /* Rounded rect, not pill */
		border: 1px solid rgba(113, 113, 122, 0.2); /* Zinc-500/20 */
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
	.deck-chip {
		color: #93c5fd;
		border-color: rgba(96, 165, 250, 0.22);
		background: rgba(96, 165, 250, 0.07);
	}
	/* Colors for types */
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

	.service-url {
		display: flex;
		align-items: center;
		gap: 5px;
		min-width: 0;
		font-size: 11px;
		font-weight: 500;
		color: #cbd5e1;
		text-decoration: none;
		font-family: var(--font-mono, monospace);
		letter-spacing: -0.01em;
		overflow: hidden;
		white-space: nowrap;
		text-overflow: ellipsis;
		padding: 3px 6px;
		border-radius: 6px;
		background: rgba(34, 211, 238, 0.07);
		border: 1px solid rgba(34, 211, 238, 0.18);
		transition:
			color 0.2s,
			background 0.2s,
			border-color 0.2s;
	}
	.service-url:hover {
		color: #f8fafc;
		background: rgba(34, 211, 238, 0.12);
		border-color: rgba(34, 211, 238, 0.35);
	}
	.service-url span {
		overflow: hidden;
		white-space: nowrap;
		text-overflow: ellipsis;
	}

	/* --- Layer 2: Toolbar Overlay --- */
	.item-toolbar {
		grid-area: stack;
		justify-self: end;
		z-index: 2;
		display: flex;
		align-items: center;
		height: 100%;
		position: relative;
		padding-left: 48px; /* Increased fade area */

		/* Visibility Logic */
		opacity: 0;
		pointer-events: none;
		transition: opacity 0.1s;
	}

	/* State A (Monitored) & State C (Hover) -> Toolbar Visible */
	.rack-item.monitored .item-toolbar,
	.rack-item:hover .item-toolbar {
		opacity: 1;
		pointer-events: auto;
	}

	/* Gradient Mask / Background */
	.toolbar-bg {
		position: absolute;
		inset: 0;
		z-index: -1;
		/* Fade to the current row background - Stronger gradient */
		background: linear-gradient(to left, var(--row-bg) 60%, transparent);
		pointer-events: none;
	}

	.toolbar-actions {
		display: flex;
		align-items: center;
		gap: 6px; /* Increased gap */
		height: 100%;
		background: transparent;
		padding-right: 4px;
	}

	.control-btn {
		background: none;
		border: none;
		color: #71717a;
		cursor: pointer;
		padding: 6px; /* Larger touch target */
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
	.control-btn:focus-visible {
		outline: 2px solid #3b82f6;
		outline-offset: 2px;
	}

	/* Monitor Icon Active State - The "Blue Glow" */
	.monitor-btn.active {
		color: #fff;
		background: linear-gradient(180deg, #60a5fa 0%, #3b82f6 100%); /* Blue-400 to Blue-500 */
		box-shadow:
			0 0 12px rgba(59, 130, 246, 0.5),
			inset 0 1px 0 rgba(255, 255, 255, 0.2);
		border: 1px solid rgba(59, 130, 246, 0.5);
	}
	.monitor-btn.active:hover {
		background: linear-gradient(180deg, #3b82f6 0%, #2563eb 100%);
		box-shadow:
			0 0 16px rgba(59, 130, 246, 0.7),
			inset 0 1px 0 rgba(255, 255, 255, 0.2);
	}

	.power-btn {
		color: #52525b;
	}
	.power-btn:hover {
		color: #ef4444;
	}

	.menu-wrapper {
		position: relative;
		display: flex;
		align-items: center;
	}

	.menu-dropdown {
		position: absolute;
		top: 100%;
		right: 0;
		background: #18181b;
		border: 1px solid #27272a;
		border-radius: 6px;
		padding: 4px;
		min-width: 140px;
		z-index: 100;
		box-shadow: 0 4px 12px rgba(0, 0, 0, 0.5);
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	.menu-item {
		padding: 6px 8px;
		font-size: 11px;
		color: #a1a1aa;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}
	.menu-item.info {
		background: #27272a;
		border-radius: 4px;
		margin-bottom: 2px;
	}

	.menu-separator {
		height: 1px;
		background: #27272a;
		margin: 2px 0;
	}

	.menu-action {
		background: none;
		border: none;
		color: #e4e4e7;
		padding: 6px 8px;
		text-align: left;
		cursor: pointer;
		border-radius: 4px;
		display: flex;
		align-items: center;
		gap: 6px;
		font-size: 12px;
	}
	.menu-action:hover {
		background: #27272a;
	}
	.menu-action:focus-visible {
		outline: 2px solid #3b82f6;
		outline-offset: -2px;
		background: #27272a;
	}
	.menu-action.danger {
		color: #ef4444;
	}
	.menu-action.danger:hover {
		background: #ef444422;
	}
	.menu-action.danger:focus-visible {
		background: #ef444422;
	}

	.rack-footer {
		padding: 16px;
		border-top: 1px solid #27272a;
		font-size: 12px;
		cursor: pointer;
		transition: background 0.2s;
	}
	.rack-footer:hover {
		background: rgba(255, 255, 255, 0.05);
	}
	.rack-footer:focus-visible {
		outline: 2px solid #3b82f6;
		outline-offset: -2px;
		background: rgba(255, 255, 255, 0.05);
	}
	.rack-footer.active {
		background: #27272a;
		border-left: 2px solid #fff;
	}

	.status-summary {
		display: flex;
		align-items: center;
		gap: 8px;
	}

	.rack-section-header {
		display: flex;
		flex-direction: column;
		align-items: stretch;
		gap: 3px;
		padding: 12px 8px 6px 8px;
		font-size: 10px;
		color: #52525b;
		border: none;
		background: none;
		width: 100%;
		cursor: pointer;
		text-align: left;
		margin-top: 4px;
		border-top: 1px solid rgba(39, 39, 42, 0.5);
	}
	.rack-section-header:first-child {
		padding-top: 8px;
		margin-top: 0;
		border-top: none;
	}
	.rack-section-header:hover {
		color: #71717a;
	}
	.rack-section-header:focus-visible {
		outline: 2px solid #3b82f6;
		outline-offset: -2px;
	}
	.section-title-row {
		display: flex;
		align-items: center;
		gap: 5px;
		font-weight: 700;
		letter-spacing: 0.05em;
		text-transform: uppercase;
	}
	.section-count {
		margin-left: auto;
		color: #71717a;
		font-weight: 600;
		letter-spacing: 0;
		text-transform: none;
	}
	.section-subtitle {
		display: block;
		padding-left: 20px;
		font-size: 10px;
		font-weight: 500;
		letter-spacing: 0;
		line-height: 1.25;
		text-transform: none;
		color: #52525b;
	}

	.section-dot {
		width: 6px;
		height: 6px;
		border-radius: 50%;
		background: #71717a;
	}
	.section-dot.active {
		background: #4ade80;
		box-shadow: 0 0 0 1px rgba(74, 222, 128, 0.2);
	}

	.rack-group-header.selected {
		background: #27272a;
	}
	.rack-group-header.selected .group-title span {
		color: #e4e4e7;
	}

	/* Context menu */
	.rack-context-menu {
		position: fixed;
		z-index: 200;
		background: #18181b;
		border: 1px solid #3f3f46;
		border-radius: 6px;
		padding: 4px;
		min-width: 160px;
		box-shadow: 0 8px 24px rgba(0, 0, 0, 0.5);
	}
	.rack-context-menu button {
		display: block;
		width: 100%;
		padding: 6px 8px;
		background: transparent;
		border: none;
		color: #ef4444;
		cursor: pointer;
		border-radius: 4px;
		text-align: left;
		font-size: 12px;
	}
	.rack-context-menu button:hover {
		background: rgba(239, 68, 68, 0.1);
	}
</style>
