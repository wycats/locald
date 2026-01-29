<script lang="ts">
	import { slide } from 'svelte/transition';
	import { WifiOff } from 'lucide-svelte';
	import { connection, isDisconnected } from '$lib/stores/connection';
	import { reconnect } from '$lib/api';
	import Spinner from './Spinner.svelte';

	let isRetrying = false;

	$: if (!$isDisconnected) {
		isRetrying = false;
	}

	function handleRetry() {
		if (isRetrying || $connection.state === 'connecting') return;
		isRetrying = true;
		reconnect();
	}
</script>

{#if $isDisconnected}
	<div class="banner" transition:slide={{ duration: 160 }}>
		<div class="content">
			<WifiOff size={18} />
			<div class="text">
				<div class="title">Connection issue</div>
				<div class="message">{$connection.errorMessage ?? 'Connection lost to server'}</div>
			</div>
		</div>
		<button
			class="retry"
			onclick={handleRetry}
			disabled={isRetrying || $connection.state === 'connecting'}
		>
			{#if isRetrying || $connection.state === 'connecting'}
				<Spinner size={14} />
				<span>Reconnecting...</span>
			{:else}
				<span>Retry</span>
			{/if}
		</button>
	</div>
{/if}

<style>
	.banner {
		position: sticky;
		top: 0;
		z-index: 40;
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 16px;
		padding: 10px 16px;
		background: #3f1d1d; /* Red-900 */
		border-bottom: 1px solid #ef4444; /* Red-500 */
		color: #f4f4f5; /* Zinc-100 */
		box-shadow: 0 6px 14px rgba(0, 0, 0, 0.35);
	}

	.content {
		display: flex;
		align-items: center;
		gap: 12px;
	}

	.text {
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	.title {
		font-weight: 600;
		font-size: 0.95rem;
	}

	.message {
		font-size: 0.85rem;
		color: #e4e4e7; /* Zinc-200 */
	}

	.retry {
		display: inline-flex;
		align-items: center;
		gap: 8px;
		padding: 6px 12px;
		border-radius: 6px;
		border: 1px solid #52525b; /* Zinc-600 */
		background: #18181b; /* Zinc-900 */
		color: #e4e4e7; /* Zinc-200 */
		font-size: 0.85rem;
		cursor: pointer;
		transition:
			border-color 0.2s,
			background 0.2s;
	}

	.retry:hover:not(:disabled) {
		background: #27272a; /* Zinc-800 */
		border-color: #3f3f46; /* Zinc-700 */
	}

	.retry:active:not(:disabled) {
		background: #18181b; /* Zinc-900 */
		border-color: #52525b; /* Zinc-600 */
	}

	.retry:disabled {
		opacity: 0.6;
		cursor: not-allowed;
	}
</style>
