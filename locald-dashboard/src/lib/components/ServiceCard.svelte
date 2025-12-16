<script lang="ts">
	import type { ServiceStatus } from '$lib/types';
	import { logs } from '$lib/stores/logs';
	import { RotateCw, Square, Play, ExternalLink, Settings } from 'lucide-svelte';
	import { startService, stopService, restartService } from '$lib/api';
	import { cleanLog } from '$lib/utils/logs';
	import AnsiToHtml from 'ansi-to-html';

	interface Props {
		service: ServiceStatus;
		onSelect: () => void;
		onConfig: () => void;
	}

	let { service, onSelect, onConfig }: Props = $props();

	const converter = new AnsiToHtml({
		escapeXML: true
	});

	// Access store value reactively
	let serviceLogs = $derived($logs[service.name] || []);
	let lastLogs = $derived(serviceLogs.slice(-3));

	async function handleStart(e: Event) {
		e.stopPropagation();
		try {
			await startService(service.name);
		} catch (err) {
			console.error(err);
		}
	}

	async function handleStop(e: Event) {
		e.stopPropagation();
		try {
			await stopService(service.name);
		} catch (err) {
			console.error(err);
		}
	}

	async function handleRestart(e: Event) {
		e.stopPropagation();
		try {
			await restartService(service.name);
		} catch (err) {
			console.error(err);
		}
	}

	function getDisplayUrl(service: ServiceStatus) {
		if (service.domain) return service.domain;
		if (service.url) {
			try {
				const url = new URL(service.url);
				if (url.hostname.endsWith('.localhost')) {
					return url.hostname;
				}
				// If it's localhost, just return the host:port
				if (url.hostname === 'localhost' || url.hostname === '127.0.0.1') {
					return url.host;
				}
			} catch {
				/* empty */
			}
			return service.url.replace(/^https?:\/\//, '');
		}
		return '';
	}
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="card" onclick={onSelect}>
	<div class="header">
		<div class="title">
			<span
				class="status-dot"
				class:running={service.status === 'running'}
				class:building={service.status === 'building'}
				class:healthy={service.health_status === 'Healthy'}
				class:starting={service.health_status === 'Starting'}
				class:unhealthy={service.health_status === 'Unhealthy'}
			></span>
			<span class="name">
				{service.name.split(':').pop()}
			</span>
		</div>
		{#if service.url}
			<!-- eslint-disable-next-line svelte/no-navigation-without-resolve -->
			<a href={service.url} target="_blank" onclick={(e) => e.stopPropagation()} class="link">
				{getDisplayUrl(service)}
				<ExternalLink size={12} />
			</a>
		{/if}
	</div>

	<div class="body">
		{#each lastLogs as log (log.timestamp + '-' + log.message)}
			<!-- eslint-disable-next-line svelte/no-at-html-tags -->
			<div class="log-line">{@html converter.toHtml(cleanLog(log.message))}</div>
		{/each}
		{#if lastLogs.length === 0}
			<div class="log-line empty">No logs yet...</div>
		{/if}
	</div>

	<div class="footer">
		<div class="actions">
			{#if service.status === 'running'}
				<button title="Restart" onclick={handleRestart}><RotateCw size={14} /></button>
				<button title="Stop" onclick={handleStop}><Square size={14} /></button>
			{:else}
				<button title="Start" onclick={handleStart}><Play size={14} /></button>
			{/if}
		</div>
		<button
			class="config-btn"
			onclick={(e) => {
				e.stopPropagation();
				onConfig();
			}}
		>
			<Settings size={14} />
		</button>
	</div>
</div>

<style>
	.card {
		background: #09090b; /* Zinc-950 */
		border: 1px solid #27272a; /* Zinc-800 */
		border-radius: 8px;
		padding: 12px;
		display: flex;
		flex-direction: column;
		gap: 8px;
		cursor: pointer;
		transition: border-color 0.2s;
	}

	.card:hover {
		border-color: #3f3f46; /* Zinc-700 */
	}

	.header {
		display: flex;
		justify-content: space-between;
		align-items: center;
	}

	.title {
		display: flex;
		align-items: center;
		gap: 8px;
		font-weight: 500;
		font-size: 0.95rem;
		color: #e4e4e7; /* Zinc-200 */
	}

	.status-dot {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		background: #71717a; /* Zinc-500 */
		box-shadow: 0 0 0 1px rgba(113, 113, 122, 0.2);
	}
	.status-dot.running {
		background: #a1a1aa; /* Zinc-400 */
		box-shadow: 0 0 0 1px rgba(161, 161, 170, 0.2);
	}
	.status-dot.healthy {
		background: #4ade80; /* Green-400 */
		box-shadow: 0 0 0 1px rgba(74, 222, 128, 0.2);
	}
	.status-dot.starting {
		background: #facc15; /* Yellow-400 */
		box-shadow: 0 0 0 1px rgba(250, 204, 21, 0.2);
	}
	.status-dot.unhealthy {
		background: #f87171; /* Red-400 */
		box-shadow: 0 0 0 1px rgba(248, 113, 113, 0.2);
	}
	.status-dot.building {
		background: #c084fc; /* Purple-400 */
		box-shadow: 0 0 0 1px rgba(192, 132, 252, 0.2);
		animation: pulse 1.5s infinite;
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

	.link {
		font-size: 0.8rem;
		color: #71717a; /* Zinc-500 */
		text-decoration: none;
		display: flex;
		align-items: center;
		gap: 4px;
		transition: color 0.15s;
	}
	.link:hover {
		color: #e4e4e7; /* Zinc-200 */
	}

	.body {
		background: #18181b; /* Zinc-900 */
		border-radius: 6px;
		padding: 8px;
		font-family: 'JetBrains Mono', monospace;
		font-size: 0.8rem;
		color: #d4d4d8; /* Zinc-300 */
		height: 60px;
		overflow: hidden;
		display: flex;
		flex-direction: column;
		justify-content: flex-end;
		border: 1px solid #27272a; /* Zinc-800 */
	}

	.log-line {
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		line-height: 1.5;
	}
	.log-line.empty {
		color: #52525b; /* Zinc-600 */
		font-style: italic;
	}

	.footer {
		display: flex;
		justify-content: space-between;
		align-items: center;
		margin-top: 4px;
	}

	.actions {
		display: flex;
		gap: 4px;
	}

	button {
		background: transparent;
		border: 1px solid #27272a; /* Zinc-800 */
		color: #a1a1aa; /* Zinc-400 */
		width: 28px;
		height: 28px;
		border-radius: 6px;
		display: flex;
		align-items: center;
		justify-content: center;
		cursor: pointer;
		transition:
			background-color 0.15s,
			color 0.15s,
			border-color 0.15s;
	}
	button:hover {
		background: #27272a; /* Zinc-800 */
		color: #e4e4e7; /* Zinc-200 */
		border-color: #3f3f46; /* Zinc-700 */
	}
	button.config-btn {
		border: none;
	}
	button.config-btn:hover {
		background: #27272a; /* Zinc-800 */
	}
</style>
