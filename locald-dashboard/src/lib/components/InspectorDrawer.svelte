<script lang="ts">
	import {
		X,
		ExternalLink,
		Terminal as TerminalIcon,
		FileText,
		AlertTriangle,
		Folder,
		Container
	} from 'lucide-svelte';
	import { getServiceInspect } from '$lib/api';
	import Terminal from './Terminal.svelte';
	import InteractiveTerminal from './InteractiveTerminal.svelte';

	interface ServiceInspectResponse {
		name: string;
		pid: number | null;
		port: number | null;
		url: string | null;
		connection_url?: string;
		health_status: string;
		health_source: string;
		path: string | null;
		container_id: string | null;
		warnings: string[];
		config?: unknown;
	}

	interface Props {
		serviceName: string | null;
		onClose: () => void;
	}

	let { serviceName, onClose }: Props = $props();

	let info: ServiceInspectResponse | null = $state(null);
	let loading = $state(false);
	let error: string | null = $state(null);
	let viewMode: 'logs' | 'terminal' = $state('logs');

	$effect(() => {
		if (serviceName) {
			loadInfo(serviceName);
		} else {
			info = null;
		}
	});

	async function loadInfo(name: string) {
		loading = true;
		error = null;
		try {
			info = (await getServiceInspect(name)) as ServiceInspectResponse;
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			loading = false;
		}
	}
</script>

{#if serviceName}
	<div class="inspector-focus">
		<div class="header">
			<div class="header-info">
				<h2>{serviceName.split(':').pop()}</h2>
				{#if info}
					<div class="status-pills">
						<span class="pill status" class:healthy={info.health_status === 'Healthy'}>
							{info.health_status}
							{#if info.health_source && info.health_source !== 'process'}
								<span class="health-source">({info.health_source})</span>
							{/if}
						</span>
						{#if info.pid}
							<span class="pill">PID: {info.pid}</span>
						{/if}
						{#if info.port}
							<span class="pill">Port: {info.port}</span>
						{/if}
						{#if info.url}
							<!-- eslint-disable-next-line svelte/no-navigation-without-resolve -->
							<a href={info.url as string} target="_blank" class="pill link">
								<ExternalLink size={12} />
								<span>{info.url}</span>
							</a>
						{/if}
						{#if info.connection_url}
							<span class="pill connection-url" title="Connection URL (click to copy)">
								<button
									class="copy-btn"
									onclick={() =>
										info?.connection_url && navigator.clipboard.writeText(info.connection_url)}
								>
									{info.connection_url}
								</button>
							</span>
						{/if}
					</div>
				{/if}
			</div>
			<div class="header-controls">
				<div class="view-toggle">
					<button
						class:active={viewMode === 'logs'}
						onclick={() => (viewMode = 'logs')}
						title="Logs"
					>
						<FileText size={16} />
					</button>
					<button
						class:active={viewMode === 'terminal'}
						onclick={() => (viewMode = 'terminal')}
						title="Terminal"
					>
						<TerminalIcon size={16} />
					</button>
				</div>
				<button onclick={onClose} aria-label="Close"><X size={20} /></button>
			</div>
		</div>

		{#if info?.warnings && info.warnings.length > 0}
			<div class="warnings-section">
				{#each info.warnings as warning, i (i)}
					<div class="warning-badge">
						<AlertTriangle size={14} />
						<span>{warning}</span>
					</div>
				{/each}
			</div>
		{/if}

		{#if info && (info.path || info.container_id)}
			<div class="metadata-section">
				{#if info.path}
					<div class="metadata-item">
						<Folder size={14} />
						<span class="path">{info.path}</span>
					</div>
				{/if}
				{#if info.container_id}
					<div class="metadata-item">
						<Container size={14} />
						<button
							class="copy-btn"
							title="Container ID (click to copy)"
							onclick={() => info?.container_id && navigator.clipboard.writeText(info.container_id)}
						>
							{info.container_id.slice(0, 12)}
						</button>
					</div>
				{/if}
			</div>
		{/if}

		<div class="content">
			{#if loading}
				<div class="loading">Loading...</div>
			{:else if error}
				<div class="error">{error}</div>
			{:else if info}
				<div class="terminal-section">
					<div class="terminal-wrapper">
						{#if viewMode === 'logs'}
							<Terminal filter={serviceName} />
						{:else}
							<InteractiveTerminal {serviceName} />
						{/if}
					</div>
				</div>
			{/if}
		</div>
	</div>
{/if}

<style>
	.inspector-focus {
		position: fixed;
		top: 0;
		left: 250px; /* Sidebar width */
		right: 0;
		bottom: 0;
		background: #1e1e1e;
		z-index: 50;
		display: flex;
		flex-direction: column;
	}

	.header {
		padding: 12px 16px;
		border-bottom: 1px solid #333;
		display: flex;
		justify-content: space-between;
		align-items: center;
		background: #252526;
	}

	.header-info {
		display: flex;
		align-items: center;
		gap: 16px;
	}

	.header-controls {
		display: flex;
		align-items: center;
		gap: 16px;
	}

	.view-toggle {
		display: flex;
		background: #333;
		border-radius: 4px;
		padding: 2px;
	}

	.view-toggle button {
		padding: 4px 8px;
		border-radius: 2px;
		color: #999;
		background: transparent;
		border: none;
		cursor: pointer;
		display: flex;
		align-items: center;
	}

	.view-toggle button:hover {
		color: #fff;
		background: #3d3d3d;
	}

	.view-toggle button.active {
		background: #444;
		color: #fff;
	}

	.header h2 {
		margin: 0;
		font-size: 1.1rem;
		font-weight: 600;
	}

	.status-pills {
		display: flex;
		gap: 8px;
		align-items: center;
	}

	.pill {
		font-size: 0.8rem;
		padding: 2px 8px;
		background: #333;
		border-radius: 4px;
		color: #ccc;
		display: flex;
		align-items: center;
		gap: 4px;
	}

	.pill.status.healthy {
		background: #1e3a1e;
		color: #4caf50;
		border: 1px solid #2e5a2e;
	}

	.pill.link {
		text-decoration: none;
		background: #2d2d2d;
		border: 1px solid #444;
	}
	.pill.link:hover {
		background: #3d3d3d;
		color: #fff;
	}

	.pill.connection-url {
		background: #2d2d2d;
		border: 1px solid #444;
		font-family: monospace;
		font-size: 0.75rem;
	}

	.pill.connection-url .copy-btn {
		background: transparent;
		border: none;
		color: inherit;
		font-family: inherit;
		font-size: inherit;
		cursor: pointer;
		padding: 0;
	}

	.pill.connection-url .copy-btn:hover {
		color: #fff;
	}

	.header button {
		background: transparent;
		border: none;
		color: #999;
		cursor: pointer;
	}
	.header button:hover {
		color: #fff;
	}

	.content {
		flex: 1;
		overflow: hidden;
		display: flex;
		flex-direction: column;
	}

	.terminal-section {
		flex: 1;
		padding: 0;
		display: flex;
		flex-direction: column;
		background: #1e1e1e;
	}

	.terminal-wrapper {
		flex: 1;
		overflow: hidden;
	}

	.loading,
	.error {
		padding: 24px;
		color: #999;
	}
	.error {
		color: #f44336;
	}

	.warnings-section {
		padding: 8px 16px;
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.warning-badge {
		display: flex;
		align-items: center;
		gap: 6px;
		padding: 4px 8px;
		background: #3d2c00;
		border: 1px solid #5c4300;
		border-radius: 4px;
		color: #ffa500;
		font-size: 0.8rem;
	}

	.metadata-section {
		padding: 8px 16px;
		display: flex;
		flex-wrap: wrap;
		gap: 12px;
		border-bottom: 1px solid #333;
		background: #252526;
	}

	.metadata-item {
		display: flex;
		align-items: center;
		gap: 6px;
		color: #999;
		font-size: 0.8rem;
	}

	.metadata-item .path {
		font-family: monospace;
		color: #ccc;
	}

	.metadata-item .copy-btn {
		background: transparent;
		border: none;
		color: #ccc;
		font-family: monospace;
		font-size: 0.8rem;
		cursor: pointer;
		padding: 0;
	}

	.metadata-item .copy-btn:hover {
		color: #fff;
	}

	.health-source {
		opacity: 0.7;
		font-size: 0.75rem;
		margin-left: 4px;
	}
</style>
