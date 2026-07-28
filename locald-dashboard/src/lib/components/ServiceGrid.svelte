<script lang="ts">
	import { serviceIdentity, type ServiceStatus } from '$lib/types';
	import ServiceCard from './ServiceCard.svelte';

	interface Props {
		services: ServiceStatus[];
		onInspect: (service: ServiceStatus) => void;
	}

	let { services, onInspect }: Props = $props();
</script>

<div class="grid">
	{#each services as service (serviceIdentity(service))}
		<ServiceCard
			{service}
			onSelect={() => onInspect(service)}
			onConfig={() => onInspect(service)}
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
