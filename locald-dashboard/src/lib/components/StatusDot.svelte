<script lang="ts">
	interface Props {
		status:
			| 'running'
			| 'stopped'
			| 'building'
			| 'healthy'
			| 'starting'
			| 'unhealthy'
			| 'connected'
			| 'disconnected'
			| 'unknown';
		size?: 'sm' | 'md';
	}

	let { status, size = 'md' }: Props = $props();
</script>

<span class="status-dot {status} {size}"></span>

<style>
	.status-dot {
		display: inline-block;
		border-radius: 50%;
		flex-shrink: 0;
	}

	.status-dot.sm {
		width: 6px;
		height: 6px;
	}

	.status-dot.md {
		width: 8px;
		height: 8px;
	}

	/* Status colors */
	.status-dot.running,
	.status-dot.healthy,
	.status-dot.connected {
		background: #22c55e;
		box-shadow: 0 0 6px rgba(34, 197, 94, 0.4);
	}

	.status-dot.starting {
		background: #facc15;
		box-shadow: 0 0 6px rgba(250, 204, 21, 0.4);
	}

	.status-dot.stopped,
	.status-dot.unhealthy,
	.status-dot.disconnected {
		background: #ef4444;
		box-shadow: 0 0 6px rgba(239, 68, 68, 0.4);
	}

	.status-dot.building {
		background: #a855f7;
		box-shadow: 0 0 6px rgba(168, 85, 247, 0.4);
		animation: pulse 1.5s ease-in-out infinite;
	}

	.status-dot.unknown {
		background: #6b7280;
		box-shadow: 0 0 6px rgba(107, 114, 128, 0.4);
	}

	@keyframes pulse {
		0%,
		100% {
			opacity: 1;
		}
		50% {
			opacity: 0.5;
		}
	}
</style>
