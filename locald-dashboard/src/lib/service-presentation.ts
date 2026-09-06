import type { PublicationState, PublicationStatus, ServiceStatus } from './types';

export function isPublishedService(service: ServiceStatus): boolean {
	return service.service_type === 'published';
}

export function managedServices(services: ServiceStatus[]): ServiceStatus[] {
	return services.filter((service) => !isPublishedService(service));
}

export function serviceDestination(service: ServiceStatus): string | null {
	return service.publication?.origin ?? service.url;
}

export function serviceDisplayName(service: ServiceStatus, projectName: string): string {
	if (service.service_name != null) return service.service_name;
	for (const separator of [':', '-', '_']) {
		const prefix = `${projectName}${separator}`;
		if (service.name.startsWith(prefix)) return service.name.slice(prefix.length);
	}
	return service.name;
}

export function serviceActionName(service: ServiceStatus): string {
	return service.instance_id ? (service.service_name ?? service.name) : service.name;
}

export function serviceDisplayAuthority(service: ServiceStatus): string {
	const origin = serviceDestination(service);
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
			return 'Waiting for app';
		case 'checking_endpoint':
			return 'Checking connection';
		case 'endpoint_unhealthy':
			return 'Unavailable';
		case 'ready':
			return 'Ready';
		case 'route_paused':
			return 'Paused';
		case 'instance_missing':
			return 'Worktree missing';
	}
}

export function serviceLifecycleLabel(service: ServiceStatus): string {
	if (service.publication) return publicationStateLabel(service.publication.state);
	if (isPublishedService(service)) return 'Status unknown';
	return service.status[0].toUpperCase() + service.status.slice(1);
}

export function serviceLifecycleSummary(services: ServiceStatus[]): string {
	if (services.length === 0) return 'No services';

	const managed = managedServices(services);
	const running = managed.filter((service) => service.status === 'running').length;
	const building = managed.filter((service) => service.status === 'building').length;
	const stopped = managed.length - running - building;
	const parts: string[] = [];

	if (managed.length > 0) parts.push(`${running} running`);
	if (building > 0) parts.push(`${building} building`);
	if (stopped > 0) parts.push(`${stopped} stopped`);
	for (const state of [
		'ready',
		'waiting_for_publisher',
		'checking_endpoint',
		'endpoint_unhealthy',
		'route_paused',
		'instance_missing'
	] as const) {
		const count = services.filter(
			(service) => isPublishedService(service) && service.publication?.state === state
		).length;
		if (count > 0) parts.push(`${count} ${publicationStateLabel(state).toLowerCase()}`);
	}
	const unknown = services.filter(
		(service) => isPublishedService(service) && !service.publication
	).length;
	if (unknown > 0) parts.push(`${unknown} status unknown`);

	return parts.join(' · ');
}

// Translate only known server defaults. Diagnostics and actions supplied by a
// newer server or an application remain verbatim, independently for each field.
const explanationCopy = new Map([
	[
		'The stable service identity is declared, but no external publisher currently fulfills it.',
		'Another application starts this service. locald is waiting for it to connect.'
	],
	[
		'The owning workflow has published an exact endpoint, but locald has not authorized it for routing yet.',
		'The application has connected. locald is checking whether the service can receive requests.'
	],
	[
		'The owning workflow is publishing this service, but its exact endpoint is unhealthy.',
		'The application has connected, but the service is failing its health check.'
	],
	[
		'The owning workflow is publishing a healthy endpoint through this stable origin.',
		'Another application runs this service. It is ready to open at this address.'
	],
	[
		'The project route is paused; locald is preserving this published origin without routing it.',
		'The project is paused. locald is keeping this address but is not sending traffic to the service.'
	],
	[
		'The worktree for this published service is missing; locald is preserving its stable origin without routing it.',
		'The worktree is missing. locald is keeping this address but is not sending traffic to the service.'
	],
	[
		'The stable service identity is declared, but locald could not verify its publisher authority.',
		'locald could not verify which application is authorized to connect this service.'
	],
	[
		'The stable service identity is declared, but locald could not verify its lifecycle state.',
		'locald could not verify whether this project allows the service to receive traffic.'
	]
]);
const nextStepCopy = new Map([
	['Start the service with its owning workflow.', 'Start the service in that application.'],
	[
		'Wait for locald to verify the published endpoint.',
		'Wait for locald to finish checking the connection.'
	],
	[
		'Inspect the owning workflow and its `/api/health` endpoint.',
		'Check the application and its /api/health endpoint.'
	],
	[
		'Resume the project to allow its owning workflow to restore publication.',
		'Resume the project to restore access.'
	],
	[
		'Inspect locald status, then retry from the owning workflow.',
		'Check locald status, then retry from the application that starts this service.'
	],
	[
		'Inspect project status, then start the service with its owning workflow.',
		'Check project status, then start the service in its application.'
	]
]);

export function publicationGuidance(
	publication: PublicationStatus
): Pick<PublicationStatus, 'explanation' | 'next_step'> {
	return {
		explanation: explanationCopy.get(publication.explanation) ?? publication.explanation,
		next_step:
			publication.next_step == null
				? publication.next_step
				: (nextStepCopy.get(publication.next_step) ?? publication.next_step)
	};
}
