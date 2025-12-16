<script lang="ts">
	import { Pin, Terminal as TerminalIcon, Activity } from 'lucide-svelte';
	import Terminal from './Terminal.svelte';
	import DaemonControlCenter from './DaemonControlCenter.svelte';
	import { fade, fly } from 'svelte/transition';
	import { cubicOut } from 'svelte/easing';

	let { pinned = $bindable([]) }: { pinned: string[] } = $props();

	function unpin(name: string) {
		pinned = pinned.filter((n) => n !== name);
	}
</script>

<div class="deck" data-testid="deck" class:single={pinned.length === 1}>
	{#each pinned as serviceName (serviceName)}
		<div
			class="terminal-card"
			in:fly={{ y: 20, duration: 300, easing: cubicOut }}
			out:fade={{ duration: 200 }}
		>
			<div class="terminal-header">
				<div class="header-left">
					{#if serviceName === 'locald'}
						<Activity size={14} class="icon locald-icon" />
						<span class="term-title">Daemon Control Center</span>
					{:else}
						<TerminalIcon size={14} class="icon" />
						<span class="term-title">{serviceName}</span>
					{/if}
				</div>
				<div class="header-right">
					<button onclick={() => unpin(serviceName)} class="action-btn" aria-label="Unpin">
						<Pin size={14} class="pinned-icon" />
					</button>
				</div>
			</div>
			<div class="terminal-body">
				{#if serviceName === 'locald'}
					<DaemonControlCenter />
				{:else}
					<Terminal filter={serviceName} />
				{/if}
			</div>
		</div>
	{/each}
</div>

<style>
	.deck {
		display: grid;
		gap: 16px;
		padding: 16px;
		height: 100%;
		background: #09090b; /* Zinc-950 */
		overflow-y: auto;
		grid-template-columns: repeat(auto-fit, minmax(min(100%, 600px), 1fr));
		align-content: start;
	}

	.deck.single {
		grid-template-columns: 1fr;
	}

	.terminal-card {
		background: #09090b;
		border: 1px solid #27272a; /* Zinc-800 */
		border-radius: 8px;
		display: flex;
		flex-direction: column;
		overflow: hidden;
		min-height: 400px;
		height: 100%;
		box-shadow:
			0 4px 6px -1px rgba(0, 0, 0, 0.1),
			0 2px 4px -1px rgba(0, 0, 0, 0.06);
		transition: border-color 0.2s ease;
	}

	.terminal-card:hover {
		border-color: #3f3f46; /* Zinc-700 */
	}

	.terminal-header {
		background: #18181b; /* Zinc-900 */
		border-bottom: 1px solid #27272a;
		padding: 0 12px;
		height: 40px;
		display: flex;
		justify-content: space-between;
		align-items: center;
		flex-shrink: 0;
	}

	.header-left {
		display: flex;
		align-items: center;
		gap: 8px;
		color: #e4e4e7; /* Zinc-200 */
		font-family: 'Inter', sans-serif;
		font-size: 13px;
		font-weight: 500;
	}

	/* Global icon class for lucide icons */
	:global(.icon) {
		color: #a1a1aa; /* Zinc-400 */
	}

	:global(.locald-icon) {
		color: #10b981; /* Emerald-500 */
	}

	.action-btn {
		background: none;
		border: none;
		color: #a1a1aa;
		cursor: pointer;
		padding: 6px;
		border-radius: 4px;
		display: flex;
		align-items: center;
		justify-content: center;
		transition: all 0.2s;
	}

	.action-btn:hover {
		background: #27272a; /* Zinc-800 */
		color: #fff;
	}

	:global(.pinned-icon) {
		fill: currentColor;
	}

	.terminal-body {
		flex: 1;
		position: relative;
		background: #09090b;
		min-height: 0;
		padding: 4px; /* Slight padding for terminal content */
	}
</style>
