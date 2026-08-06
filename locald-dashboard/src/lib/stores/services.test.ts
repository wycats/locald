import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';
import { resolveServiceSelector, type ServiceStatus } from '$lib/types';
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

afterEach(() => {
	vi.unstubAllGlobals();
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

	it('translates a unique legacy monitor label to the stable service identity', () => {
		expect(resolveServiceSelector(base.name, [base])).toBe(
			`${base.instance_id}/${base.service_name}`
		);
	});

	it('keeps an ambiguous legacy monitor label unresolved', () => {
		const second = {
			...base,
			instance_id: '00000000-0000-0000-0000-000000000002',
			path: '/second'
		};

		expect(resolveServiceSelector(base.name, [base, second])).toBe(base.name);
	});

	it('refetches a full list when a newer service update races the first response', async () => {
		let resolveFirstFetch: ((response: Response) => void) | undefined;
		const fetchMock = vi
			.fn()
			.mockImplementationOnce(
				() =>
					new Promise<Response>((resolve) => {
						resolveFirstFetch = resolve;
					})
			)
			.mockResolvedValueOnce(
				new Response(JSON.stringify([base]), {
					status: 200,
					headers: { 'content-type': 'application/json' }
				})
			);
		vi.stubGlobal('fetch', fetchMock);

		const refresh = services.refresh();
		await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledOnce());
		services.updateService(base);
		resolveFirstFetch?.(
			new Response(JSON.stringify([{ ...base, status: 'stopped' }]), {
				status: 200,
				headers: { 'content-type': 'application/json' }
			})
		);
		await refresh;

		expect(fetchMock).toHaveBeenCalledTimes(2);
		expect(get(services)).toEqual([base]);
	});

	it('preserves a newer refresh request when the active request fails', async () => {
		let rejectFirstFetch: ((error: Error) => void) | undefined;
		const fetchMock = vi
			.fn()
			.mockImplementationOnce(
				() =>
					new Promise<Response>((_resolve, reject) => {
						rejectFirstFetch = reject;
					})
			)
			.mockResolvedValueOnce(
				new Response(JSON.stringify([base]), {
					status: 200,
					headers: { 'content-type': 'application/json' }
				})
			);
		vi.stubGlobal('fetch', fetchMock);

		const firstRefresh = services.refresh();
		await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledOnce());
		const newerRefresh = services.refresh();
		rejectFirstFetch?.(new Error('connection lost'));
		await Promise.all([firstRefresh, newerRefresh]);

		expect(fetchMock).toHaveBeenCalledTimes(2);
		expect(get(services)).toEqual([base]);
	});
});
