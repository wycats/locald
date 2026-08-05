import { writable, derived } from 'svelte/store';
import { serviceIdentity, type ServiceStatus, type ServiceMetrics } from '$lib/types';
import { getServices } from '$lib/api';

export const servicesLoading = writable<boolean>(true);
export const servicesError = writable<string | null>(null);

function createServicesStore() {
	const { subscribe, set: replace, update: mutate } = writable<ServiceStatus[]>([]);
	let stateRevision = 0;
	let refreshRequested = false;
	let activeRefresh: Promise<void> | null = null;

	function recordStateChange() {
		stateRevision += 1;
		if (activeRefresh) {
			refreshRequested = true;
		}
	}

	async function runRefreshes() {
		servicesLoading.set(true);
		servicesError.set(null);
		try {
			while (refreshRequested) {
				refreshRequested = false;
				const revision = stateRevision;
				try {
					const nextServices = await getServices();
					servicesError.set(null);
					if (revision === stateRevision) {
						replace(nextServices);
					} else {
						refreshRequested = true;
					}
				} catch (error) {
					const message = error instanceof Error ? error.message : 'Failed to load services.';
					servicesError.set(message);
				}
			}
		} finally {
			servicesLoading.set(false);
			activeRefresh = null;
		}
	}

	function refresh() {
		refreshRequested = true;
		if (!activeRefresh) {
			activeRefresh = runRefreshes();
		}
		return activeRefresh;
	}

	return {
		subscribe,
		set: (nextServices: ServiceStatus[]) => {
			recordStateChange();
			replace(nextServices);
		},
		update: (updater: (services: ServiceStatus[]) => ServiceStatus[]) => {
			recordStateChange();
			mutate(updater);
		},
		refresh,
		updateService: (updatedService: ServiceStatus) => {
			recordStateChange();
			mutate((services) => {
				const identity = serviceIdentity(updatedService);
				const index = services.findIndex((s) => serviceIdentity(s) === identity);
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
				recordStateChange();
				mutate((services) => {
					const identity = serviceIdentity(updatedService);
					const index = services.findIndex((s) => serviceIdentity(s) === identity);
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
				recordStateChange();
				mutate((services) => {
					const identity = serviceIdentity(metrics);
					const index = services.findIndex((s) => serviceIdentity(s) === identity);
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
	const groups = new Map<
		string,
		{ name: string; path: string | null; services: ServiceStatus[] }
	>();
	for (const service of $services) {
		let groupName: string | null = null;

		if (service.workspace && service.constellation) {
			groupName = `${service.workspace}/${service.constellation}`;
		} else {
			groupName = service.workspace || service.constellation;
		}

		if (!groupName) {
			// Assuming format "project:service"
			const parts = service.name.split(':');
			groupName = parts.length > 1 ? parts[0] : 'default';
		}

		const groupKey = service.path ?? groupName;
		const group = groups.get(groupKey) ?? {
			name: groupName,
			path: service.path,
			services: []
		};
		group.services.push(service);
		groups.set(groupKey, group);
	}
	return [...groups.values()];
});
