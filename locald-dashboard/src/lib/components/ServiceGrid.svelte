<script lang="ts">
	import type { ServiceStatus } from '$lib/types';
	import ServiceCard from './ServiceCard.svelte';

	interface Props {
		services: ServiceStatus[];
		onInspect: (name: string) => void;
	}

	let { services, onInspect }: Props = $props();
</script>

<div class="grid">
	{#each services as service (service.name)}
		<ServiceCard
			{service}
			onSelect={() => onInspect(service.name)}
			onConfig={() => onInspect(service.name)}
		/>
	{/each}
</div>

<style>
	.grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
		gap: 16px;
		padding: 16px;
	}
</style>
