<script lang="ts">
	import { X, ExternalLink } from 'lucide-svelte';
	import { getServiceInspect } from '$lib/api';
	import Terminal from './Terminal.svelte';

	interface Props {
		serviceName: string | null;
		onClose: () => void;
	}

	let { serviceName, onClose }: Props = $props();

	let info: Record<string, unknown> | null = $state(null);
	let loading = $state(false);
	let error: string | null = $state(null);

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
			info = (await getServiceInspect(name)) as Record<string, unknown>;
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
					</div>
				{/if}
			</div>
			<button onclick={onClose} aria-label="Close"><X size={20} /></button>
		</div>

		<div class="content">
			{#if loading}
				<div class="loading">Loading...</div>
			{:else if error}
				<div class="error">{error}</div>
			{:else if info}
				<div class="terminal-section">
					<div class="terminal-wrapper">
						<Terminal filter={serviceName} />
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
</style>
