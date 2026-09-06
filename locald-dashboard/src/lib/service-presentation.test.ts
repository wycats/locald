import { describe, expect, it } from 'vitest';
import {
	isPublishedService,
	managedServices,
	publicationStateLabel,
	publicationGuidance,
	serviceDisplayAuthority,
	serviceDestination,
	serviceDisplayName,
	serviceActionName,
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

describe('service identity and destinations', () => {
	it('targets the exact service name within its instance and retains legacy selectors', () => {
		expect(serviceActionName(service({ service_name: 'web:admin' }))).toBe('web:admin');
		expect(serviceActionName(service({ instance_id: null, service_name: 'web:admin' }))).toBe(
			'example:web'
		);
	});

	it('uses the exact declared service name, including colons and project prefixes', () => {
		expect(serviceDisplayName(service({ service_name: 'example:web:admin' }), 'example')).toBe(
			'example:web:admin'
		);
		expect(serviceDisplayName(service({ service_name: 'exampletools' }), 'example')).toBe(
			'exampletools'
		);
	});

	it('only trims a separated project prefix from legacy qualified names', () => {
		expect(
			serviceDisplayName(service({ service_name: null, name: 'example:web:admin' }), 'example')
		).toBe('web:admin');
		expect(
			serviceDisplayName(service({ service_name: null, name: 'exampletools' }), 'example')
		).toBe('exampletools');
	});

	it('retains supplied stopped-service destinations and sandbox ports', () => {
		const stopped = service({
			status: 'stopped',
			url: 'https://example.localhost:8443/path?query=1'
		});
		expect(serviceDestination(stopped)).toBe(stopped.url);
		expect(serviceDisplayAuthority(stopped)).toBe('example.localhost:8443');
	});

	it('leaves workers and database services without an invented web destination', () => {
		for (const service_type of ['worker', 'postgres'] as const) {
			expect(
				serviceDestination(
					service({ service_type, url: null, connection_url: 'postgres://localhost/db' })
				)
			).toBeNull();
		}
	});
});

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
		expect(serviceLifecycleLabel(published)).toBe('Waiting for app');
		expect(publicationStateLabel('route_paused')).toBe('Paused');
		expect(publicationStateLabel('instance_missing')).toBe('Worktree missing');
	});

	it('uses the canonical publication origin for both navigation and display', () => {
		expect(serviceDestination(published)).toBe('https://workbench.example.localhost');
		expect(serviceDisplayAuthority(published)).toBe('workbench.example.localhost');
	});

	it('summarizes published readiness independently from managed runtime state', () => {
		expect(serviceLifecycleSummary([service(), published])).toBe('1 running · 1 waiting for app');
		expect(serviceLifecycleSummary([published])).toBe('1 waiting for app');
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

describe('plain service readiness and guidance', () => {
	const cases = [
		{
			state: 'waiting_for_publisher',
			label: 'Waiting for app',
			explanation:
				'The stable service identity is declared, but no external publisher currently fulfills it.',
			next_step: 'Start the service with its owning workflow.',
			expected: {
				explanation:
					'Another application starts this service. locald is waiting for it to connect.',
				next_step: 'Start the service in that application.'
			}
		},
		{
			state: 'checking_endpoint',
			label: 'Checking connection',
			explanation:
				'The owning workflow has published an exact endpoint, but locald has not authorized it for routing yet.',
			next_step: 'Wait for locald to verify the published endpoint.',
			expected: {
				explanation:
					'The application has connected. locald is checking whether the service can receive requests.',
				next_step: 'Wait for locald to finish checking the connection.'
			}
		},
		{
			state: 'endpoint_unhealthy',
			label: 'Unavailable',
			explanation:
				'The owning workflow is publishing this service, but its exact endpoint is unhealthy.',
			next_step: 'Inspect the owning workflow and its `/api/health` endpoint.',
			expected: {
				explanation: 'The application has connected, but the service is failing its health check.',
				next_step: 'Check the application and its /api/health endpoint.'
			}
		},
		{
			state: 'ready',
			label: 'Ready',
			explanation:
				'The owning workflow is publishing a healthy endpoint through this stable origin.',
			next_step: undefined,
			expected: {
				explanation: 'Another application runs this service. It is ready to open at this address.',
				next_step: undefined
			}
		},
		{
			state: 'route_paused',
			label: 'Paused',
			explanation:
				'The project route is paused; locald is preserving this published origin without routing it.',
			next_step: 'Resume the project to allow its owning workflow to restore publication.',
			expected: {
				explanation:
					'The project is paused. locald is keeping this address but is not sending traffic to the service.',
				next_step: 'Resume the project to restore access.'
			}
		},
		{
			state: 'instance_missing',
			label: 'Worktree missing',
			explanation:
				'The worktree for this published service is missing; locald is preserving its stable origin without routing it.',
			next_step:
				'Restore the worktree, or explicitly forget the project if this identity is no longer needed.',
			expected: {
				explanation:
					'The worktree is missing. locald is keeping this address but is not sending traffic to the service.',
				next_step:
					'Restore the worktree, or explicitly forget the project if this identity is no longer needed.'
			}
		}
	] as const;
	it.each(cases)(
		'labels and translates $state server defaults',
		({ state, label, explanation, next_step, expected }) => {
			const publication = { state, origin: 'https://example.localhost', explanation, next_step };
			expect(publicationStateLabel(state)).toBe(label);
			expect(publicationGuidance(publication)).toEqual(expected);
		}
	);
	it('retains authority verification failure meaning', () => {
		expect(
			publicationGuidance({
				state: 'waiting_for_publisher',
				origin: 'https://example.localhost',
				explanation:
					'The stable service identity is declared, but locald could not verify its publisher authority.',
				next_step: 'Inspect locald status, then retry from the owning workflow.'
			})
		).toEqual({
			explanation:
				'locald could not verify which application is authorized to connect this service.',
			next_step: 'Check locald status, then retry from the application that starts this service.'
		});
	});
	it('retains lifecycle verification failure meaning', () => {
		expect(
			publicationGuidance({
				state: 'waiting_for_publisher',
				origin: 'https://example.localhost',
				explanation:
					'The stable service identity is declared, but locald could not verify its lifecycle state.',
				next_step: 'Inspect project status, then start the service with its owning workflow.'
			})
		).toEqual({
			explanation:
				'locald could not verify whether this project allows the service to receive traffic.',
			next_step: 'Check project status, then start the service in its application.'
		});
	});
	it('preserves custom fields independently, including embedded jargon', () => {
		for (const item of cases) {
			const publication = { ...item, origin: 'https://example.localhost' };
			const custom = 'Diagnostic: publisher rejected endpoint; run custom --retry.';
			expect(publicationGuidance({ ...publication, explanation: custom })).toEqual({
				explanation: custom,
				next_step: item.expected.next_step
			});
			expect(publicationGuidance({ ...publication, next_step: custom })).toEqual({
				explanation: item.expected.explanation,
				next_step: custom
			});
			expect(
				publicationGuidance({ ...publication, next_step: undefined }).next_step
			).toBeUndefined();
		}
	});
	it('summarizes actual states and keeps managed runtime counts separate', () => {
		const published = cases.map((item) =>
			service({
				service_type: 'published',
				status: 'externally_managed',
				publication: { ...item, origin: 'https://example.localhost' }
			})
		);
		const unknown = service({ service_type: 'published', status: 'externally_managed' });
		expect(serviceLifecycleLabel(unknown)).toBe('Status unknown');
		expect(serviceLifecycleSummary([])).toBe('No services');
		expect(serviceLifecycleSummary([unknown])).toBe('1 status unknown');
		const managed = [service(), service({ status: 'building' }), service({ status: 'stopped' })];
		expect(managed.map(serviceLifecycleLabel)).toEqual(['Running', 'Building', 'Stopped']);
		expect(serviceLifecycleSummary(managed)).toBe('1 running · 1 building · 1 stopped');
		const summary =
			'1 ready · 1 waiting for app · 1 checking connection · 1 unavailable · 1 paused · 1 worktree missing';
		expect(serviceLifecycleSummary(published)).toBe(summary);
		expect(serviceLifecycleSummary([...managed, ...published, unknown])).toBe(
			'1 running · 1 building · 1 stopped · ' + summary + ' · 1 status unknown'
		);
	});
});
