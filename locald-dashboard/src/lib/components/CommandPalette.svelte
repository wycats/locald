<script lang="ts">
	import { Search, Play, Square, RotateCw, Trash2 } from 'lucide-svelte';
	import { fade, fly } from 'svelte/transition';
	import { services } from '$lib/stores/services';
	import {
		startService,
		stopService,
		restartService,
		stopAllServices,
		restartAllServices
	} from '$lib/api';

	let isOpen = $state(false);
	let query = $state('');
	let selectedIndex = $state(0);
	let inputElement = $state<HTMLInputElement>();

	type Command = {
		id: string;
		title: string;
		action: () => void;
		icon?: any;
	};

	let commands = $derived.by(() => {
		const list: Command[] = [];

		// Global commands
		list.push({
			id: 'restart-all',
			title: 'Restart All Services',
			action: () => restartAllServices(),
			icon: RotateCw
		});
		list.push({
			id: 'stop-all',
			title: 'Stop All Services',
			action: () => stopAllServices(),
			icon: Square
		});

		// Service specific commands
		for (const service of $services) {
			const name = service.name;

			if (service.status === 'running') {
				list.push({
					id: `restart-${name}`,
					title: `Restart ${name}`,
					action: () => restartService(name),
					icon: RotateCw
				});
				list.push({
					id: `stop-${name}`,
					title: `Stop ${name}`,
					action: () => stopService(name),
					icon: Square
				});
			} else {
				list.push({
					id: `start-${name}`,
					title: `Start ${name}`,
					action: () => startService(name),
					icon: Play
				});
			}
		}

		return list;
	});

	let filteredCommands = $derived(
		commands.filter((c) => c.title.toLowerCase().includes(query.toLowerCase()))
	);

	export function open() {
		isOpen = true;
		query = '';
		selectedIndex = 0;
		// Focus input after render
		setTimeout(() => inputElement?.focus(), 10);
	}

	export function close() {
		isOpen = false;
	}

	function execute(command: Command) {
		command.action();
		close();
	}

	function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'k' && (e.metaKey || e.ctrlKey)) {
			e.preventDefault();
			if (isOpen) close();
			else open();
		}

		if (!isOpen) return;

		if (e.key === 'Escape') {
			close();
		} else if (e.key === 'ArrowDown') {
			e.preventDefault();
			selectedIndex = (selectedIndex + 1) % filteredCommands.length;
		} else if (e.key === 'ArrowUp') {
			e.preventDefault();
			selectedIndex = (selectedIndex - 1 + filteredCommands.length) % filteredCommands.length;
		} else if (e.key === 'Enter') {
			e.preventDefault();
			if (filteredCommands[selectedIndex]) {
				execute(filteredCommands[selectedIndex]);
			}
		}
	}
</script>

<svelte:window onkeydown={handleKeydown} />

{#if isOpen}
	<div class="overlay" transition:fade={{ duration: 100 }}>
		<div class="backdrop" onclick={close} role="presentation"></div>

		<div class="palette" transition:fly={{ y: 10, duration: 100 }}>
			<div class="search-bar">
				<Search class="icon" size={20} />
				<input
					bind:this={inputElement}
					bind:value={query}
					placeholder="Type a command..."
					spellcheck="false"
				/>
			</div>

			<div class="results">
				{#if filteredCommands.length === 0}
					<div class="empty">No commands found.</div>
				{:else}
					{#each filteredCommands as command, i}
						<button
							class="command-item"
							class:selected={i === selectedIndex}
							onclick={() => execute(command)}
							onmouseenter={() => (selectedIndex = i)}
						>
							{#if command.icon}
								<command.icon class="command-icon" size={16} />
							{/if}
							<span class="command-title">{command.title}</span>
						</button>
					{/each}
				{/if}
			</div>

			<div class="footer">
				<span>↑↓ to navigate</span>
				<span>↵ to select</span>
				<span>esc to close</span>
			</div>
		</div>
	</div>
{/if}

<style>
	.overlay {
		position: fixed;
		inset: 0;
		z-index: 9999;
		display: flex;
		justify-content: center;
		align-items: flex-start;
		padding-top: 20vh;
		font-family: var(--font-sans);
	}

	.backdrop {
		position: fixed;
		inset: 0;
		background: rgba(0, 0, 0, 0.5);
		backdrop-filter: blur(2px);
	}

	.palette {
		position: relative;
		width: 100%;
		max-width: 600px;
		background: #18181b; /* zinc-900 */
		border: 1px solid #27272a; /* zinc-800 */
		border-radius: 12px;
		box-shadow: 0 20px 25px -5px rgba(0, 0, 0, 0.1), 0 10px 10px -5px rgba(0, 0, 0, 0.04);
		overflow: hidden;
		display: flex;
		flex-direction: column;
	}

	.search-bar {
		display: flex;
		align-items: center;
		padding: 16px;
		border-bottom: 1px solid #27272a; /* zinc-800 */
	}

	.search-bar :global(.icon) {
		color: #71717a; /* zinc-500 */
		margin-right: 12px;
	}

	input {
		flex: 1;
		background: transparent;
		border: none;
		font-size: 18px;
		color: #f4f4f5; /* zinc-100 */
		outline: none;
	}

	input::placeholder {
		color: #71717a; /* zinc-500 */
	}

	.results {
		max-height: 300px;
		overflow-y: auto;
		padding: 8px 0;
	}

	.empty {
		padding: 24px;
		text-align: center;
		color: #71717a; /* zinc-500 */
	}

	.command-item {
		width: 100%;
		display: flex;
		align-items: center;
		padding: 12px 16px;
		background: transparent;
		border: none;
		text-align: left;
		cursor: pointer;
		color: #a1a1aa; /* zinc-400 */
		transition: all 0.1s;
	}

	.command-item.selected {
		background: #27272a; /* zinc-800 */
		color: #f4f4f5; /* zinc-100 */
	}

	.command-item :global(.command-icon) {
		margin-right: 12px;
	}

	.footer {
		display: flex;
		gap: 16px;
		padding: 8px 16px;
		background: #09090b; /* zinc-950 */
		border-top: 1px solid #27272a; /* zinc-800 */
		font-size: 12px;
		color: #52525b; /* zinc-600 */
	}
</style>
