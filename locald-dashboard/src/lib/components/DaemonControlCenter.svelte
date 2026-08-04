<script lang="ts">
	import { onMount } from 'svelte';
	import { projects, services } from '$lib/stores/services';
	import { restartAllServices } from '$lib/api';
	import Terminal from './Terminal.svelte';
	import { managedServices } from '$lib/service-presentation';

	let sseConnected = $state<boolean | null>(null);
	let restarting = $state(false);

	async function handleRestartAll() {
		restarting = true;
		try {
			await restartAllServices();
		} catch {
			// Silently handle — services store will update via SSE
		} finally {
			setTimeout(() => (restarting = false), 2000);
		}
	}

	let projectsCount = $derived($projects.length);
	let managed = $derived(managedServices($services));
	let publishedCount = $derived($services.length - managed.length);
	let runningCount = $derived(managed.filter((s) => s.status === 'running').length);
	let buildingCount = $derived(managed.filter((s) => s.status === 'building').length);
	let stoppedCount = $derived(managed.filter((s) => s.status === 'stopped').length);

	onMount(() => {
		const tick = () => {
			if (typeof document === 'undefined') return;
			const attr = document.body?.getAttribute('data-sse-connected');
			sseConnected = attr === 'true' ? true : attr === 'false' ? false : null;
		};

		tick();
		const timer = setInterval(tick, 500);
		return () => clearInterval(timer);
	});
</script>

<div class="dcc" data-testid="daemon-control-center">
	<div class="grid">
		<div class="card">
			<div class="card-title">Connection</div>
			<div class="conn">
				<span class="dot" class:ok={sseConnected === true} class:bad={sseConnected === false}
				></span>
				<span class="conn-text">
					{#if sseConnected === true}
						Connected
					{:else if sseConnected === false}
						Disconnected
					{:else}
						Unknown
					{/if}
				</span>
			</div>
			<div class="muted">
				This view adds the daemon to the Deck as a virtual service (“System Normal”).
			</div>
		</div>

		<div class="card">
			<div class="card-header">
				<div class="card-title">Workspace Summary</div>
				<button
					class="restart-btn"
					onclick={handleRestartAll}
					disabled={restarting || runningCount === 0}
					title="Restart all running managed services"
				>
					{restarting ? 'Restarting…' : 'Restart Managed'}
				</button>
			</div>
			<div class="summary">
				<div class="stat">
					<div class="stat-num">{projectsCount}</div>
					<div class="stat-label">Projects</div>
				</div>
				<div class="stat">
					<div class="stat-num">{managed.length}</div>
					<div class="stat-label">Managed</div>
				</div>
				<div class="stat">
					<div class="stat-num">{publishedCount}</div>
					<div class="stat-label">Published</div>
				</div>
				<div class="stat">
					<div class="stat-num">{runningCount}</div>
					<div class="stat-label">Running</div>
				</div>
				<div class="stat">
					<div class="stat-num">{buildingCount}</div>
					<div class="stat-label">Building</div>
				</div>
				<div class="stat">
					<div class="stat-num">{stoppedCount}</div>
					<div class="stat-label">Stopped</div>
				</div>
			</div>
		</div>
	</div>

	<div class="terminal">
		<div class="terminal-title">Recent activity</div>
		<!-- Best-effort: show something useful even if daemon logs aren't tagged as "locald". -->
		<Terminal filter={null} textFilter="" />
	</div>
</div>

<style>
	.dcc {
		display: flex;
		flex-direction: column;
		height: 100%;
		gap: 12px;
		padding: 12px;
	}

	.grid {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
		gap: 12px;
	}

	.card {
		background: #09090b;
		border: 1px solid #27272a;
		border-radius: 8px;
		padding: 12px;
		min-height: 96px;
	}

	.card-header {
		display: flex;
		justify-content: space-between;
		align-items: center;
		margin-bottom: 8px;
	}

	.card-title {
		font-size: 12px;
		font-weight: 600;
		color: #e4e4e7;
	}

	.restart-btn {
		font-size: 11px;
		padding: 3px 8px;
		border-radius: 4px;
		border: 1px solid #3f3f46;
		background: #18181b;
		color: #a1a1aa;
		cursor: pointer;
		transition: all 0.15s;
	}

	.restart-btn:hover:not(:disabled) {
		background: #27272a;
		color: #e4e4e7;
		border-color: #52525b;
	}

	.restart-btn:disabled {
		opacity: 0.4;
		cursor: not-allowed;
	}

	.muted {
		margin-top: 8px;
		font-size: 12px;
		color: #a1a1aa;
		line-height: 1.3;
	}

	.conn {
		display: flex;
		align-items: center;
		gap: 8px;
		font-size: 13px;
		color: #d4d4d8;
	}

	.dot {
		width: 10px;
		height: 10px;
		border-radius: 999px;
		background: #52525b; /* Zinc-600 */
	}

	.dot.ok {
		background: #10b981; /* Emerald-500 */
	}

	.dot.bad {
		background: #ef4444; /* Red-500 */
	}

	.summary {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(90px, 1fr));
		gap: 10px;
	}

	.stat {
		border: 1px solid #27272a;
		border-radius: 8px;
		padding: 10px;
		background: #09090b;
	}

	.stat-num {
		font-size: 18px;
		font-weight: 700;
		color: #f4f4f5;
		line-height: 1;
	}

	.stat-label {
		margin-top: 4px;
		font-size: 11px;
		color: #a1a1aa;
	}

	.terminal {
		flex: 1;
		min-height: 0;
		background: #09090b;
		border: 1px solid #27272a;
		border-radius: 8px;
		overflow: hidden;
		display: flex;
		flex-direction: column;
	}

	.terminal-title {
		padding: 8px 10px;
		font-size: 12px;
		font-weight: 600;
		color: #e4e4e7;
		background: #18181b;
		border-bottom: 1px solid #27272a;
	}

	/* Ensure the terminal fills the remaining space */
	.terminal :global(.terminal-container) {
		flex: 1;
		min-height: 0;
	}
</style>
