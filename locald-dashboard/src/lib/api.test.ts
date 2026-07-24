import { afterEach, describe, expect, it, vi } from 'vitest';
import { connectEvents, reconnect } from './api';
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
});
