<script lang="ts">
	/* eslint-disable svelte/no-navigation-without-resolve */
	import { projects, services, servicesError, servicesLoading } from '$lib/stores/services';
	import { pendingActions } from '$lib/stores/actions';
	import {
		startServiceWithFeedback,
		stopServiceWithFeedback,
		restartServiceWithFeedback,
		resetServiceWithFeedback
	} from '$lib/actions/service';
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
		AlertCircle
	} from 'lucide-svelte';
	import type { ServiceStatus } from '$lib/types';
	import Spinner from './Spinner.svelte';

	export let monitored: string[] = [];

	let collapsedGroups: string[] = [];
	let activeMenu: string | null = null;
	let focused: string | null = null;

	$: allServices = $projects.flatMap((p) => p.services);

	function handleKeydown(event: KeyboardEvent) {
		if (event.target instanceof HTMLInputElement || event.target instanceof HTMLTextAreaElement)
			return;

		if (event.key === 'j' || event.key === 'ArrowDown') {
			moveFocus(1);
		} else if (event.key === 'k' || event.key === 'ArrowUp') {
			moveFocus(-1);
		} else if (event.key === 'Enter' || event.key === ' ') {
			event.preventDefault();
			if (focused) toggleMonitor(focused);
		} else if (event.key === 'Escape') {
			monitored = [];
			activeMenu = null;
		}
	}

	function moveFocus(direction: number) {
		if (allServices.length === 0) return;

		let currentIndex = -1;
		if (focused) {
			currentIndex = allServices.findIndex((s) => s.name === focused);
		}

		let nextIndex = currentIndex + direction;

		// Wrap around or clamp? Let's clamp.
		if (nextIndex < 0) nextIndex = 0;
		if (nextIndex >= allServices.length) nextIndex = allServices.length - 1;

		focused = allServices[nextIndex].name;

		// Ensure visible
		ensureVisible(focused);
	}

	function ensureVisible(name: string) {
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

		if (monitored.includes(name)) {
			monitored = monitored.filter((n) => n !== name);
		} else {
			monitored = [...monitored, name];
		}
	}

	function toggleMonitorGroup(groupName: string, groupServices: ServiceStatus[]) {
		const serviceNames = groupServices.map((s) => s.name);
		const allMonitored = serviceNames.every((name) => monitored.includes(name));

		if (allMonitored) {
			monitored = monitored.filter((n) => !serviceNames.includes(n));
		} else {
			monitored = [...new Set([...monitored, ...serviceNames])];
		}
	}

	function toggleMenu(name: string, event: Event) {
		event.stopPropagation();
		activeMenu = activeMenu === name ? null : name;
	}

	function closeMenu() {
		activeMenu = null;
	}

	function isPending(serviceName: string): boolean {
		return $pendingActions.some((a) => a.serviceName === serviceName);
	}

	async function toggleGroup(groupServices: ServiceStatus[]) {
		const allStopped = groupServices.every((s) => s.status === 'stopped');
		await Promise.all(
			groupServices.map((s) => {
				if (allStopped) return startServiceWithFeedback(s.name);
				return stopServiceWithFeedback(s.name);
			})
		);
	}

	function toggleGroupCollapse(group: string) {
		if (collapsedGroups.includes(group)) {
			collapsedGroups = collapsedGroups.filter((g) => g !== group);
		} else {
			collapsedGroups = [...collapsedGroups, group];
		}
	}

	function toggleSystemMonitor() {
		const name = 'locald';
		if (monitored.includes(name)) {
			monitored = monitored.filter((n) => n !== name);
		} else {
			monitored = [...monitored, name];
		}
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
</script>

<svelte:window on:keydown={handleKeydown} />

<!-- a11y: Click-to-close has keyboard alternative via Escape key in handleKeydown -->
<!-- svelte-ignore a11y-click-events-have-key-events -->
<!-- svelte-ignore a11y-no-noninteractive-element-interactions -->
<div class="rack" on:click={closeMenu} role="application">
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
		{:else if $projects.length === 0}
			<div class="rack-state empty">
				<Layers size={24} />
				<span class="state-title">No services found</span>
				<span class="state-message">
					Run <code>locald up</code> in a project directory to start services.
				</span>
			</div>
		{:else}
			{#each $projects as project (project.name)}
				{@const isCollapsed = collapsedGroups.includes(project.name)}
				{@const isAllStopped = project.services.every((s) => s.status === 'stopped')}

				<div class="rack-group-header" class:disabled={isAllStopped}>
					<button
						class="group-title"
						type="button"
						on:click={() => toggleGroupCollapse(project.name)}
					>
						{#if isCollapsed}
							<ChevronRight size={12} />
						{:else}
							<ChevronDown size={12} />
						{/if}
						<span>{project.name}</span>
					</button>
					<div class="group-actions">
						<button
							class="group-btn"
							on:click|stopPropagation={() => toggleMonitorGroup(project.name, project.services)}
							title="Monitor group in Deck"
						>
							<Layers size={12} />
						</button>
						<button
							class="group-btn"
							on:click|stopPropagation={() => toggleGroup(project.services)}
							title={isAllStopped ? 'Start Group' : 'Stop Group'}
						>
							<Power size={12} color={isAllStopped ? '#52525b' : '#ef4444'} />
						</button>
					</div>
				</div>

				{#if !isCollapsed}
					{#each project.services as service (service.name)}
						{@const type = getServiceType(service)}
						{@const displayName = getDisplayName(service.name, project.name)}

						<div
							id="service-{service.name}"
							class="rack-item"
							class:monitored={monitored.includes(service.name)}
							class:focused={focused === service.name}
							class:disabled={service.status === 'stopped'}
							on:click={() => toggleMonitor(service.name)}
							on:keydown={(e) => {
								if (e.key === 'Enter' || e.key === ' ') {
									e.preventDefault();
									toggleMonitor(service.name);
								}
							}}
							role="button"
							tabindex="0"
						>
							<!-- Layer 1: Content (Left Group) -->
							<div class="item-content">
								<div class="status-dot {service.status}"></div>
								<span class="service-name" title={service.name}>{displayName}</span>

								{#if service.url && service.status === 'running'}
									<a
										href={service.url}
										target="_blank"
										class="type-chip {type} interactive"
										title="Open {service.url}"
										on:click={(e) => e.stopPropagation()}
									>
										{type}
										<ExternalLink size={9} />
									</a>
								{:else}
									<span class="type-chip {type}">{type}</span>
								{/if}
							</div>

							<!-- Layer 2: Toolbar Overlay -->
							<div class="item-toolbar">
								<div class="toolbar-bg"></div>
								<div class="toolbar-actions">
									{#if service.status === 'running'}
										<button
											class="control-btn monitor-btn"
											class:active={monitored.includes(service.name)}
											on:click={(e) => toggleMonitor(service.name, e)}
											title="Monitor in Deck"
										>
											<Monitor size={14} />
										</button>
										<button
											class="control-btn"
											title="Restart"
											disabled={isPending(service.name)}
											on:click|stopPropagation={() => restartServiceWithFeedback(service.name)}
										>
											{#if isPending(service.name)}
												<Spinner size={14} />
											{:else}
												<RefreshCw size={14} />
											{/if}
										</button>
										<div class="menu-wrapper">
											<button
												class="control-btn"
												on:click={(e) => toggleMenu(service.name, e)}
												title="More"
											>
												<MoreHorizontal size={14} />
											</button>
											{#if activeMenu === service.name}
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
														disabled={isPending(service.name)}
														on:click={() => resetServiceWithFeedback(service.name)}
													>
														{#if isPending(service.name)}
															<Spinner size={12} />
														{:else}
															<RotateCcw size={12} />
														{/if}
														Reset
													</button>
													<button
														class="menu-action danger"
														disabled={isPending(service.name)}
														on:click={() => stopServiceWithFeedback(service.name)}
													>
														{#if isPending(service.name)}
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
											disabled={isPending(service.name)}
											on:click|stopPropagation={() => startServiceWithFeedback(service.name)}
											title="Start"
										>
											{#if isPending(service.name)}
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
		padding: 8px 16px;
		font-size: 11px;
		text-transform: uppercase;
		color: #a1a1aa;
		font-weight: bold;
		letter-spacing: 0.05em;
		margin-top: 8px;
		display: flex;
		justify-content: space-between;
		align-items: center;
		cursor: pointer;
	}
	.rack-group-header.disabled {
		opacity: 0.5;
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
	}
	.group-title:focus-visible {
		outline: 2px solid #3b82f6;
		outline-offset: 2px;
		border-radius: 4px;
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

	/* --- Rack Item Layout --- */
	.rack-item {
		--row-bg: #09090b; /* Default: Zinc-950 */

		display: grid;
		grid-template-areas: 'stack';
		grid-template-columns: 100%;
		grid-template-rows: 100%;
		align-items: center;
		padding: 0 12px;
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

	.rack-item.focused {
		/* Fallback if not monitored/hovered, but focused usually implies one of those or keyboard nav */
		/* Let's keep it simple and match hover for focus to ensure gradient works */
		--row-bg: #27272a;
	}
	.rack-item.focused:not(.monitored) {
		border-left: 2px solid #52525b;
	}

	/* --- Layer 1: Content --- */
	.item-content {
		grid-area: stack;
		display: flex;
		align-items: center;
		gap: 8px; /* gap-2 */
		width: 100%;
		min-width: 0;
		z-index: 1;
		padding-right: 64px; /* pr-16 to avoid overlap */
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

	/* State A (Monitored) & State C (Hover) -> Bright Text */
	.rack-item.monitored .service-name,
	.rack-item:hover .service-name {
		color: #ffffff; /* White */
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
</style>
