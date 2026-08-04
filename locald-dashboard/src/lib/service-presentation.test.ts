import { describe, expect, it } from 'vitest';
import {
	isPublishedService,
	managedServices,
	publicationStateLabel,
	serviceDisplayAuthority,
	serviceLifecycleLabel,
	serviceLifecycleSummary
} from './service-presentation';
import type { ServiceStatus } from './types';

function service(overrides: Partial<ServiceStatus> = {}): ServiceStatus {
	return {
		name: 'example:web',
		instance_id: '00000000-0000-0000-0000-000000000001',
		service_name: 'web',
		service_type: 'exec',
		pid: 42,
		port: 3000,
		status: 'running',
		url: 'https://example.localhost',
		connection_url: null,
		domain: 'example.localhost',
		health_status: 'Healthy',
		health_source: 'Tcp',
		path: '/example',
		workspace: null,
		constellation: null,
		warnings: [],
		...overrides
	};
}

describe('published service presentation', () => {
	const published = service({
		name: 'example:workbench',
		service_name: 'workbench',
		service_type: 'published',
		pid: null,
		port: null,
		status: 'externally_managed',
		connection_url: null,
		health_status: 'Unknown',
		health_source: 'Http',
		publication: {
			state: 'waiting_for_publisher',
			origin: 'https://workbench.example.localhost',
			explanation: 'Waiting for an external owner.',
			next_step: 'Start it through the owning workflow.'
		}
	});

	it('keeps published services out of managed lifecycle collections', () => {
		expect(isPublishedService(published)).toBe(true);
		expect(managedServices([service(), published])).toEqual([service()]);
	});

	it('uses the publication state instead of pretending the process is stopped', () => {
		expect(serviceLifecycleLabel(published)).toBe('Waiting for publisher');
		expect(publicationStateLabel('route_paused')).toBe('Route paused');
		expect(publicationStateLabel('instance_missing')).toBe('Worktree missing');
	});

	it('counts published identities independently from managed runtime state', () => {
		expect(serviceLifecycleSummary([service(), published])).toBe('1 running · 1 published');
		expect(serviceLifecycleSummary([published])).toBe('1 published');
	});

	it('preserves explicit sandbox HTTPS ports in displayed origins', () => {
		const sandbox = {
			...published,
			url: 'https://workbench.example.localhost:8443',
			publication: {
				...published.publication!,
				origin: 'https://workbench.example.localhost:8443'
			}
		};
		expect(serviceDisplayAuthority(sandbox)).toBe('workbench.example.localhost:8443');
	});
});
