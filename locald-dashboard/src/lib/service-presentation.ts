import type { PublicationState, ServiceStatus } from './types';

export function isPublishedService(service: ServiceStatus): boolean {
	return service.service_type === 'published';
}

export function managedServices(services: ServiceStatus[]): ServiceStatus[] {
	return services.filter((service) => !isPublishedService(service));
}

export function serviceDisplayAuthority(service: ServiceStatus): string {
	const origin = service.publication?.origin ?? service.url;
	if (origin) {
		try {
			return new URL(origin).host;
		} catch {
			return origin.replace(/^https?:\/\//, '');
		}
	}
	return service.domain ?? '';
}

export function publicationStateLabel(state: PublicationState): string {
	switch (state) {
		case 'waiting_for_publisher':
			return 'Waiting for publisher';
		case 'checking_endpoint':
			return 'Checking endpoint';
		case 'endpoint_unhealthy':
			return 'Endpoint unhealthy';
		case 'ready':
			return 'Ready';
		case 'route_paused':
			return 'Route paused';
		case 'instance_missing':
			return 'Worktree missing';
	}
}

export function serviceLifecycleLabel(service: ServiceStatus): string {
	if (service.publication) return publicationStateLabel(service.publication.state);
	return service.status[0].toUpperCase() + service.status.slice(1);
}

export function serviceLifecycleSummary(services: ServiceStatus[]): string {
	if (services.length === 0) return 'No services';

	const managed = managedServices(services);
	const published = services.length - managed.length;
	const running = managed.filter((service) => service.status === 'running').length;
	const building = managed.filter((service) => service.status === 'building').length;
	const stopped = managed.length - running - building;
	const parts: string[] = [];

	if (managed.length > 0) parts.push(`${running} running`);
	if (building > 0) parts.push(`${building} building`);
	if (stopped > 0) parts.push(`${stopped} stopped`);
	if (published > 0) parts.push(`${published} published`);

	return parts.join(' · ');
}
