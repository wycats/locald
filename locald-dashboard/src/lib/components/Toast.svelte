<script lang="ts">
	import { toasts } from '$lib/stores/toasts';
	import { X, CheckCircle, AlertCircle, Info } from 'lucide-svelte';
</script>

<div class="toast-container">
	{#each $toasts as toast (toast.id)}
		<div class="toast {toast.type}">
			{#if toast.type === 'success'}
				<CheckCircle size={16} />
			{:else if toast.type === 'error'}
				<AlertCircle size={16} />
			{:else}
				<Info size={16} />
			{/if}
			<span>{toast.message}</span>
			<button on:click={() => toasts.remove(toast.id)} aria-label="Dismiss notification">
				<X size={14} />
			</button>
		</div>
	{/each}
</div>

<style>
	.toast-container {
		position: fixed;
		bottom: 16px;
		right: 16px;
		display: flex;
		flex-direction: column;
		gap: 8px;
		z-index: 1000;
	}
	.toast {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 12px 16px;
		border-radius: 8px;
		background: #27272a;
		color: #e4e4e7;
		font-size: 14px;
		box-shadow: 0 4px 12px rgba(0, 0, 0, 0.3);
	}
	.toast.success {
		border-left: 3px solid #4ade80;
	}
	.toast.error {
		border-left: 3px solid #f87171;
	}
	.toast.info {
		border-left: 3px solid #60a5fa;
	}
	.toast button {
		background: none;
		border: none;
		color: #71717a;
		cursor: pointer;
		padding: 2px;
		display: flex;
		align-items: center;
	}
	.toast button:hover {
		color: #a1a1aa;
	}
</style>
