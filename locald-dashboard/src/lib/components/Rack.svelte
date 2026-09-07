<script lang="ts">
	import { projects, services, servicesError, servicesLoading } from '$lib/stores/services';
	import {
		projectList,
		activeProjects,
		alwaysOnProjects,
		recentProjects
	} from '$lib/stores/projects';
	import { removeProject, type ProjectListEntry } from '$lib/api';
	import { toasts } from '$lib/stores/toasts';
	import { availabilityLabel } from '$lib/availability';
	import {
		Activity,
		Layers,
		ChevronRight,
		ChevronDown,
		RefreshCw,
		AlertCircle
	} from 'lucide-svelte';
	import { serviceIdentity } from '$lib/types';
	import {
		isPublishedService,
		managedServices,
		serviceDestination,
		serviceDisplayName,
		serviceLifecycleLabel
	} from '$lib/service-presentation';
	import { buildRackEntries, type ProjectSection, type RackEntry } from '$lib/rack-projects';
	import ServiceDestination from './ServiceDestination.svelte';
	import Spinner from './Spinner.svelte';

	export let monitored: string[] = [];
	export let selectedProject: string | null = null;
	export let onSelectProject: (path: string | null) => void = () => {};
	export let onToggleMonitor: (name: string | string[]) => void = () => {};

	let collapsedGroups: string[] = [];
	$: clearAttachedGroupCollapse($projectList);

	function clearAttachedGroupCollapse(entries: ProjectListEntry[]) {
		const retained = collapsedGroups.filter(
			(key) => !entries.some((entry) => entry.project_path === key)
		);
		if (retained.length !== collapsedGroups.length) collapsedGroups = retained;
	}

	let keyboardFocus: string | null = null;
	let contextMenu: { x: number; y: number; project: ProjectListEntry } | null = null;

	let collapsedSections: ProjectSection[] = ['Recent'];
	$: selectedRecentPath =
		$recentProjects.find((project) => project.project_path === selectedProject)?.project_path ??
		null;
	// Reveal the selection when it enters Recent; keep manual collapse available.
	$: if (selectedRecentPath) revealRecent();

	function revealRecent() {
		collapsedSections = collapsedSections.filter((section) => section !== 'Recent');
	}

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

	$: allServices = managedServices(
		rackEntries.flatMap((entry) =>
			entry.kind === 'project' && !collapsedGroups.includes(entry.key) ? entry.services : []
		)
	);

	// Build an ordered list: section headers interleaved with project groups
	$: rackEntries = buildRackEntries(
		$activeProjects,
		$alwaysOnProjects,
		$recentProjects,
		$projects,
		collapsedSections
	);

	function rackEntryKey(entry: RackEntry): string {
		if (entry.kind === 'section') return `section:${entry.section}`;
		return `project:${entry.key}`;
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
		if (event.key === 'Escape') {
			closeContextMenu();
			return;
		}
		if (event.defaultPrevented || event.altKey || event.ctrlKey || event.metaKey) return;
		const target = event.target instanceof Element ? event.target : null;
		const control = target?.closest(
			'a, button, summary, input, textarea, select, [contenteditable], [role="textbox"]'
		);
		if (control && !control.classList.contains('service-inspect')) return;

		if (event.key === 'j' || event.key === 'ArrowDown') {
			event.preventDefault();
			moveFocus(1);
		} else if (event.key === 'k' || event.key === 'ArrowUp') {
			event.preventDefault();
			moveFocus(-1);
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
		setTimeout(() => {
			const el = document.getElementById(`inspect-${name}`);
			if (el) {
				el.focus({ preventScroll: true });
				el.scrollIntoView({ block: 'nearest' });
			}
		}, 0);
	}

	function toggleMonitor(name: string, event?: Event) {
		if (event) event.stopPropagation();
		onToggleMonitor(name);
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

	function projectCountLabel(count: number): string {
		return `${count} project${count === 1 ? '' : 's'}`;
	}
</script>

<svelte:window on:keydown={handleKeydown} />

<!-- a11y: Click-to-close has keyboard alternative via Escape key in handleKeydown -->
<!-- svelte-ignore a11y-click-events-have-key-events -->
<!-- svelte-ignore a11y-no-noninteractive-element-interactions -->
<div
	class="rack"
	on:click={() => {
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
						title={sectionCopy.subtitle}
						aria-expanded={!sectionCollapsed}
						on:click={() => toggleSectionCollapse(rackEntry.section)}
					>
						<div class="section-title-row">
							{#if sectionCollapsed}<ChevronRight size={10} />{:else}<ChevronDown size={10} />{/if}
							<span>{sectionCopy.label}</span>
							{#if rackEntry.section === 'Recent'}<span class="section-count"
									>· {projectCountLabel(rackEntry.count)}</span
								>{/if}
						</div>
					</button>
				{:else}
					{@const project = rackEntry}
					{@const isCollapsed = collapsedGroups.includes(project.key)}
					{@const availability = project.entry?.availability}
					{@const projectIssue =
						availability && ['failed', 'degraded', 'missing'].includes(availability.state)}
					<!-- svelte-ignore a11y-no-static-element-interactions -->
					<div
						class="rack-group-header"
						data-project-path={project.path}
						class:disabled={availability != null && !availability.desired}
						class:selected={project.path != null && selectedProject === project.path}
						on:contextmenu={(e) => project.entry && handleProjectContextMenu(e, project.entry)}
					>
						<div class="group-main">
							<button
								class="group-title"
								type="button"
								aria-pressed={project.entry ? selectedProject === project.path : undefined}
								aria-expanded={project.entry ? undefined : !isCollapsed}
								title={project.path ?? project.name}
								on:click={() =>
									project.entry
										? onSelectProject(selectedProject === project.path ? null : project.path)
										: toggleGroupCollapse(project.key)}
							>
								<span>{project.name}</span>
							</button>
							{#if project.checkoutLabel}
								<span
									class="checkout-label"
									data-testid="checkout-label"
									title={project.path ?? undefined}
								>
									{project.checkoutLabel}
								</span>
							{/if}
							{#if projectIssue || project.services.length === 0}
								<div
									class="group-issue"
									class:failed={availability?.state === 'failed' ||
										availability?.state === 'degraded'}
								>
									{availabilityLabel(availability)}{project.services.length === 0
										? ' · no services'
										: ''}
								</div>
							{/if}
						</div>
					</div>

					{#if !isCollapsed && project.services.length > 0}
						<div
							class="project-services"
							role="group"
							aria-label="Services for {project.name}{project.checkoutLabel
								? ` · ${project.checkoutLabel}`
								: ''}"
						>
							{#each project.services as service (serviceIdentity(service))}
								{@const identity = serviceIdentity(service)}
								{@const displayName = serviceDisplayName(service, project.name)}
								{@const destination = serviceDestination(service)}
								{@const published = isPublishedService(service)}
								{@const inDeck = !published && monitored.includes(identity)}

								<div
									id="service-{identity}"
									class="rack-item"
									data-testid="service-row"
									data-service-key={identity}
									class:monitored={inDeck}
									class:published
									class:keyboard-focused={keyboardFocus === identity}
									class:disabled={!published && service.status === 'stopped'}
								>
									{#if published}
										<div class="item-content">
											<span class="service-name" title={service.name}>{displayName}</span>
											<span class="service-state publication-state"
												>{serviceLifecycleLabel(service)}</span
											>
										</div>
									{:else}
										<button
											type="button"
											id="inspect-{identity}"
											class="item-content service-inspect"
											aria-label="Inspect {service.name}"
											aria-pressed={inDeck}
											title={inDeck
												? `Remove ${service.name} from Deck`
												: `Inspect ${service.name} in Deck`}
											on:focus={() => (keyboardFocus = identity)}
											on:click={(event) => toggleMonitor(identity, event)}
										>
											<span class="service-name" title={service.name}>{displayName}</span>
											<span class="service-state">{serviceLifecycleLabel(service)}</span>
										</button>
									{/if}
									<ServiceDestination
										{identity}
										serviceName={service.name}
										{destination}
										{service}
									/>
								</div>
							{/each}
						</div>
					{/if}
				{/if}
			{/each}
		{/if}
	</div>

	<button
		class="rack-footer"
		class:active={monitored.includes('locald')}
		on:click={toggleSystemMonitor}
		aria-pressed={monitored.includes('locald')}
	>
		<div class="status-summary">
			<Activity size={16} />
			<span>System Normal</span>
		</div>
	</button>

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
		background: #09090b;
		border-right: 1px solid #27272a;
		display: flex;
		flex-direction: column;
		height: 100%;
		min-height: 0;
	}
	.rack-header {
		padding: 16px 20px;
		border-bottom: 1px solid #27272a;
		font-weight: 600;
		color: #e4e4e7;
	}
	.rack-list {
		flex: 1;
		overflow-y: auto;
		overflow-x: hidden;
		min-height: 0;
		padding-bottom: 20px;
	}
	.rack-section-header {
		display: block;
		width: 100%;
		padding: 20px 20px 8px;
		border: 0;
		background: none;
		color: #92929d;
		text-align: left;
		cursor: pointer;
	}
	.section-title-row {
		display: flex;
		align-items: center;
		gap: 5px;
		font-size: 11px;
		font-weight: 500;
		letter-spacing: 0.04em;
		text-transform: uppercase;
	}
	.section-count {
		text-transform: none;
		letter-spacing: 0;
	}
	.rack-group-header {
		padding: 4px 8px 1px;
		margin-top: 20px;
		margin-inline: 12px;
		border-radius: 4px;
	}
	.rack-section-header + .rack-group-header {
		margin-top: 0;
	}
	.group-main {
		display: flex;
		flex-direction: column;
		gap: 2px;
		min-width: 0;
	}
	.group-title {
		background: none;
		border: 0;
		padding: 0;
		color: #e4e4e7;
		line-height: 1.4;
		font-family: inherit;
		font-size: 14px;
		font-weight: 600;
		text-align: left;
		cursor: pointer;
		min-width: 0;
	}
	.group-title span {
		overflow-wrap: anywhere;
	}
	.rack-group-header.disabled .group-title {
		color: #a1a1aa;
	}
	.checkout-label {
		color: #a1a1aa;
		font-family: var(--font-mono, monospace);
		font-size: 11px;
		overflow-wrap: anywhere;
	}
	.group-issue {
		color: #a1a1aa;
		font-size: 11px;
	}
	.failed {
		color: #ef9999;
	}
	.project-services {
		margin: 0 12px 0 20px;
	}
	.rack-item {
		position: relative;
		display: grid;
		grid-template-columns: minmax(0, 1fr) auto;
		gap: 12px;
		align-items: center;
		min-height: 32px;
		padding: 1px 4px 1px 12px;
		border-radius: 4px;
	}
	.rack-item::before {
		content: '';
		position: absolute;
		left: 0;
		top: 0;
		bottom: 0;
		border-left: 1px solid #34343b;
		pointer-events: none;
	}
	.rack-item::after {
		content: '';
		position: absolute;
		left: 0;
		top: 50%;
		width: 5px;
		border-top: 1px solid #34343b;
		pointer-events: none;
	}
	.rack-item:last-child::before {
		bottom: 50%;
	}
	.rack-item.monitored,
	.rack-item.keyboard-focused {
		background: #1b283b;
	}
	.rack-item:hover {
		background: #18181b;
	}
	.rack-item.monitored:hover,
	.rack-item.keyboard-focused:hover {
		background: #1b283b;
	}
	.item-content {
		display: grid;
		grid-template-columns: minmax(0, 1fr) auto;
		align-items: center;
		gap: 10px;
		min-width: 0;
		width: 100%;
		min-height: 30px;
	}
	.service-inspect {
		border: 0;
		padding: 0;
		border-radius: 3px;
		background: none;
		color: inherit;
		font: inherit;
		text-align: left;
		cursor: pointer;
	}
	.service-name {
		font-size: 13px;
		font-weight: 500;
		color: #d4d4d8;
		overflow-wrap: anywhere;
	}
	.service-state {
		font-size: 11px;
		line-height: 1.4;
		font-weight: 400;
		color: #a1a1aa;
		text-align: right;
		max-width: 100px;
	}
	.rack-footer {
		background: none;
		border: 0;
		border-top: 1px solid #27272a;
		padding: 16px;
		color: #e4e4e7;
		cursor: pointer;
		text-align: left;
	}
	.rack-footer.active,
	.rack-group-header.selected {
		background: #1b283b;
	}
	.status-summary {
		display: flex;
		gap: 8px;
		align-items: center;
	}
	.rack-state {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 8px;
		padding: 24px;
		color: #a1a1aa;
		text-align: center;
	}
	.state-title {
		font-weight: 600;
	}
	.state-message {
		font-size: 12px;
	}
	.retry-btn {
		display: flex;
		gap: 6px;
		background: #27272a;
		color: #e4e4e7;
		border: 1px solid #3f3f46;
		padding: 6px 10px;
		border-radius: 4px;
		cursor: pointer;
	}
	.rack-context-menu {
		position: fixed;
		z-index: 200;
		background: #18181b;
		border: 1px solid #3f3f46;
		border-radius: 6px;
		padding: 4px;
	}
	.rack-context-menu button {
		background: none;
		border: 0;
		padding: 8px;
		color: #ef9999;
		cursor: pointer;
	}
	button:focus-visible {
		outline: 2px solid #60a5fa;
		outline-offset: 2px;
	}
	@media (max-width: 640px) {
		.rack {
			max-height: 50vh;
			border-right: 0;
			border-bottom: 1px solid #27272a;
		}
	}
</style>
