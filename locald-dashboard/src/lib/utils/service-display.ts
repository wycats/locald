import type { ServiceStatus } from '$lib/types';

export type ServiceEndpoint = {
	kind: 'public' | 'connection';
	label: string;
	value: string;
};

type ServiceEndpointSource = Pick<ServiceStatus, 'domain' | 'url' | 'connection_url'>;

function displayPublicUrl(domain: string | null, url: string): string {
	if (domain) return domain;

	try {
		const parsed = new URL(url);
		return parsed.hostname.endsWith('.localhost') ? parsed.hostname : parsed.host;
	} catch {
		return url.replace(/^https?:\/\//, '');
	}
}

export function displayServiceEndpoint(service: ServiceEndpointSource): ServiceEndpoint | null {
	if (service.url) {
		return {
			kind: 'public',
			label: displayPublicUrl(service.domain, service.url),
			value: service.url
		};
	}

	if (service.connection_url) {
		return {
			kind: 'connection',
			label: service.connection_url,
			value: service.connection_url
		};
	}

	return null;
}
