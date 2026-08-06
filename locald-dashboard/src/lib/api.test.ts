import { get } from 'svelte/store';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { connectEvents, reconnect, restartService } from './api';
import { services } from './stores/services';
import type { ServiceStatus } from './types';

class FakeEventSource {
	static readonly CLOSED = 2;
	static instances: FakeEventSource[] = [];

	readonly url: string;
	readyState = 1;
	onmessage: ((event: MessageEvent<string>) => void) | null = null;
	onopen: (() => void) | null = null;
	onerror: ((event: Event) => void) | null = null;

	constructor(url: string | URL) {
		this.url = url.toString();
		FakeEventSource.instances.push(this);
	}

	close() {
		this.readyState = FakeEventSource.CLOSED;
	}
}

const service: ServiceStatus = {
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
	path: '/example',
	workspace: null,
	constellation: null,
	warnings: []
};

afterEach(() => {
	FakeEventSource.instances = [];
	services.set([]);
	vi.unstubAllGlobals();
});

describe('dashboard lifecycle events', () => {
	it('preserves the project refresh callback across a manual SSE reconnect', () => {
		vi.stubGlobal('EventSource', FakeEventSource);
		const lifecycleChange = vi.fn();
		const disconnect = connectEvents(lifecycleChange);

		reconnect();
		const reconnected = FakeEventSource.instances.at(-1);
		expect(reconnected?.url).toBe('/api/events');
		reconnected?.onmessage?.(
			new MessageEvent('message', {
				data: JSON.stringify({ type: 'ServiceUpdate', data: service })
			})
		);

		expect(lifecycleChange).toHaveBeenCalledOnce();
		disconnect();
	});

	it('refreshes the authoritative service list after a published projection change', async () => {
		vi.stubGlobal('EventSource', FakeEventSource);
		const fetchMock = vi.fn().mockResolvedValue(
			new Response(JSON.stringify([]), {
				status: 200,
				headers: { 'content-type': 'application/json' }
			})
		);
		vi.stubGlobal('fetch', fetchMock);
		const lifecycleChange = vi.fn();
		const disconnect = connectEvents(lifecycleChange);

		FakeEventSource.instances.at(-1)?.onmessage?.(
			new MessageEvent('message', {
				data: JSON.stringify({ type: 'ServiceListChanged' })
			})
		);

		await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledWith('/api/state'));
		expect(lifecycleChange).toHaveBeenCalledOnce();
		disconnect();
	});

	it('refreshes the authoritative service list when an SSE connection opens', async () => {
		vi.stubGlobal('EventSource', FakeEventSource);
		const fetchMock = vi.fn().mockResolvedValue(
			new Response(JSON.stringify([]), {
				status: 200,
				headers: { 'content-type': 'application/json' }
			})
		);
		vi.stubGlobal('fetch', fetchMock);
		const lifecycleChange = vi.fn();
		const disconnect = connectEvents(lifecycleChange);

		FakeEventSource.instances.at(-1)?.onopen?.();

		await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledWith('/api/state'));
		expect(lifecycleChange).toHaveBeenCalledOnce();
		disconnect();
	});

	it('applies newer service updates after an in-flight authoritative refresh', async () => {
		vi.stubGlobal('EventSource', FakeEventSource);
		let resolveFetch: ((response: Response) => void) | undefined;
		const fetchMock = vi
			.fn()
			.mockImplementationOnce(
				() =>
					new Promise<Response>((resolve) => {
						resolveFetch = resolve;
					})
			)
			.mockResolvedValueOnce(
				new Response(JSON.stringify([service]), {
					status: 200,
					headers: { 'content-type': 'application/json' }
				})
			);
		vi.stubGlobal('fetch', fetchMock);
		const disconnect = connectEvents();
		const source = FakeEventSource.instances.at(-1);

		source?.onmessage?.(
			new MessageEvent('message', {
				data: JSON.stringify({ type: 'ServiceListChanged' })
			})
		);
		await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledWith('/api/state'));
		source?.onmessage?.(
			new MessageEvent('message', {
				data: JSON.stringify({ type: 'ServiceUpdate', data: service })
			})
		);
		resolveFetch?.(
			new Response(JSON.stringify([{ ...service, status: 'stopped' }]), {
				status: 200,
				headers: { 'content-type': 'application/json' }
			})
		);

		await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(2));
		await vi.waitFor(() => expect(get(services)[0]?.status).toBe('running'));
		disconnect();
	});

	it('targets service lifecycle actions by project instance', async () => {
		const fetchMock = vi.fn().mockResolvedValue(new Response(null, { status: 200 }));
		vi.stubGlobal('fetch', fetchMock);

		await restartService(service.name, service.instance_id);

		expect(fetchMock).toHaveBeenCalledWith(
			'/api/instances/00000000-0000-0000-0000-000000000001/services/example%3Aweb/restart',
			{ method: 'POST' }
		);
	});
});
