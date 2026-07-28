import { beforeEach, describe, expect, it } from 'vitest';
import { get } from 'svelte/store';
import type { ServiceStatus } from '$lib/types';
import { projects, services } from './services';

const base: ServiceStatus = {
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
	health_status: 'healthy',
	health_source: 'tcp',
	path: '/first',
	workspace: null,
	constellation: null,
	warnings: []
};

beforeEach(() => {
	services.set([]);
});

describe('instance-scoped service projections', () => {
	it('keeps identical display names from separate worktrees independent', () => {
		const second = {
			...base,
			instance_id: '00000000-0000-0000-0000-000000000002',
			path: '/second',
			port: 4000
		};

		services.updateService(base);
		services.updateService(second);
		services.updateService({ ...second, status: 'stopped', pid: null });

		expect(get(services)).toEqual([base, { ...second, status: 'stopped', pid: null }]);
		expect(get(projects)).toEqual([
			{ name: 'example', path: '/first', services: [base] },
			{
				name: 'example',
				path: '/second',
				services: [{ ...second, status: 'stopped', pid: null }]
			}
		]);
	});

	it('applies metrics only to their owning instance', () => {
		const second = {
			...base,
			instance_id: '00000000-0000-0000-0000-000000000002',
			path: '/second',
			port: 4000
		};
		services.set([base, second]);

		services.handleEvent({
			type: 'Metrics',
			data: {
				name: second.name,
				instance_id: second.instance_id,
				service_name: second.service_name,
				cpu_percent: 25,
				memory_bytes: 1024,
				timestamp: 1
			}
		});

		const [firstProjection, secondProjection] = get(services);
		expect(firstProjection.metrics).toBeUndefined();
		expect(secondProjection.metrics?.cpu_percent).toBe(25);
	});

	it('replaces the same service projection when its project display name changes', () => {
		services.set([base]);

		const renamed = { ...base, name: 'renamed:web' };
		services.updateService(renamed);

		expect(get(services)).toEqual([renamed]);
	});
});
