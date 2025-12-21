import { writable, derived } from 'svelte/store';
import type { ServiceStatus, ServiceMetrics } from '$lib/types';
import { getServices } from '$lib/api';

function createServicesStore() {
	const { subscribe, set, update } = writable<ServiceStatus[]>([]);

	return {
		subscribe,
		set,
		update,
		refresh: async () => {
			const services = await getServices();
			set(services);
		},
		updateService: (updatedService: ServiceStatus) => {
			update((services) => {
				const index = services.findIndex((s) => s.name === updatedService.name);
				if (index !== -1) {
					const newServices = [...services];
					newServices[index] = updatedService;
					return newServices;
				} else {
					const newServices = [...services, updatedService];
					return newServices.sort((a, b) => a.name.localeCompare(b.name));
				}
			});
		},
		handleEvent: (event: { type: string; data: unknown }) => {
			if (event.type === 'ServiceUpdate') {
				const updatedService = event.data as ServiceStatus;
				update((services) => {
					const index = services.findIndex((s) => s.name === updatedService.name);
					if (index !== -1) {
						const newServices = [...services];
						// Preserve metrics/history across status updates
						const oldService = newServices[index];
						newServices[index] = {
							...updatedService,
							metrics: oldService.metrics,
							cpu_history: oldService.cpu_history
						};
						return newServices;
					} else {
						const newServices = [...services, updatedService];
						return newServices.sort((a, b) => a.name.localeCompare(b.name));
					}
				});
			} else if (event.type === 'Metrics') {
				const metrics = event.data as ServiceMetrics;
				update((services) => {
					const index = services.findIndex((s) => s.name === metrics.name);
					if (index !== -1) {
						const newServices = [...services];
						const oldService = newServices[index];
						const history = oldService.cpu_history
							? [...oldService.cpu_history]
							: Array(20).fill(0);

						history.push(metrics.cpu_percent);
						if (history.length > 20) history.shift();

						newServices[index] = {
							...oldService,
							metrics,
							cpu_history: history
						};
						return newServices;
					}
					return services;
				});
			}
		}
	};
}

export const services = createServicesStore();

// Derived store for grouping by project/workspace
export const projects = derived(services, ($services) => {
	const groups: Record<string, ServiceStatus[]> = {};
	for (const service of $services) {
		let groupName = service.workspace || service.constellation;

		if (!groupName) {
			// Assuming format "project:service"
			const parts = service.name.split(':');
			groupName = parts.length > 1 ? parts[0] : 'default';
		}

		if (!groups[groupName]) {
			groups[groupName] = [];
		}
		groups[groupName].push(service);
	}
	return Object.entries(groups).map(([name, services]) => ({
		name,
		services
	}));
});
