<script lang="ts">
	import { Search } from 'lucide-svelte';
	import Terminal from '$lib/components/Terminal.svelte';

	let filterText = $state('');
</script>

<div class="stream">
	<div class="stream-header">
		<div class="search-wrapper">
			<Search size={14} class="search-icon" />
			<input
				type="text"
				placeholder="Filter logs..."
				class="search-input"
				bind:value={filterText}
			/>
		</div>
	</div>
	<div class="log-container">
		<Terminal filter={null} textFilter={filterText} />
	</div>
</div>

<style>
	.stream {
		display: flex;
		flex-direction: column;
		height: 100%;
		background: #09090b;
	}

	.stream-header {
		padding: 12px 16px;
		border-bottom: 1px solid #27272a;
		display: flex;
		align-items: center;
		background: #09090b;
	}

	.search-wrapper {
		display: flex;
		align-items: center;
		gap: 8px;
		background: #18181b;
		border: 1px solid #27272a;
		border-radius: 6px;
		padding: 6px 10px;
		width: 100%;
		max-width: 400px;
		transition: border-color 0.2s;
	}

	.search-wrapper:focus-within {
		border-color: #3f3f46;
	}

	:global(.search-icon) {
		color: #71717a;
	}

	.search-input {
		background: none;
		border: none;
		color: #e4e4e7;
		outline: none;
		font-family: 'Inter', sans-serif;
		font-size: 13px;
		width: 100%;
	}

	.search-input::placeholder {
		color: #71717a;
	}

	.log-container {
		flex: 1;
		overflow: hidden;
		padding: 8px;
		background: #09090b;
	}
</style>
