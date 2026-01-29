<script lang="ts">
	import { Loader2 } from 'lucide-svelte';

	interface Props {
		size?: number;
	}

	let { size = 14 }: Props = $props();
</script>

<Loader2 {size} class="spinner" />

<style>
	:global(.spinner) {
		animation: spin 1s linear infinite;
	}

	@keyframes spin {
		from {
			transform: rotate(0deg);
		}
		to {
			transform: rotate(360deg);
		}
	}
</style>
