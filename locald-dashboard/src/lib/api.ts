import type { ServiceStatus } from './types';

export async function getServices(): Promise<ServiceStatus[]> {
	const res = await fetch('/api/state');
	if (!res.ok) {
		throw new Error('Failed to fetch services');
	}
	return res.json();
}

export async function startService(name: string): Promise<void> {
	const res = await fetch(`/api/services/${name}/start`, { method: 'POST' });
	if (!res.ok) {
		throw new Error(`Failed to start service ${name}`);
	}
}

export async function stopService(name: string): Promise<void> {
	const res = await fetch(`/api/services/${name}/stop`, { method: 'POST' });
	if (!res.ok) {
		throw new Error(`Failed to stop service ${name}`);
	}
}

export async function restartService(name: string): Promise<void> {
	const res = await fetch(`/api/services/${name}/restart`, { method: 'POST' });
	if (!res.ok) {
		throw new Error(`Failed to restart service ${name}`);
	}
}

export async function stopAllServices(): Promise<void> {
	const res = await fetch('/api/services/stop-all', { method: 'POST' });
	if (!res.ok) {
		throw new Error('Failed to stop all services');
	}
}

export async function restartAllServices(): Promise<void> {
	const res = await fetch('/api/services/restart-all', { method: 'POST' });
	if (!res.ok) {
		throw new Error('Failed to restart all services');
	}
}

import { services } from '$lib/stores/services';
import { logs } from '$lib/stores/logs';

export async function getServiceInspect(name: string): Promise<unknown> {
	const res = await fetch(`/api/services/${name}`);
	if (!res.ok) {
		throw new Error(`Failed to inspect service ${name}`);
	}
	return res.json();
}

export function connectEvents() {
	const eventSource = new EventSource('/api/events');

	eventSource.onmessage = (event) => {
		try {
			const msg = JSON.parse(event.data);
			if (msg.type === 'Log') {
				logs.addLog(msg.data);
			} else if (msg.type === 'ServiceUpdate') {
				console.log('Received ServiceUpdate:', msg.data.name, msg.data.status);
				services.updateService(msg.data);
			}
		} catch (e) {
			console.error('Failed to parse event', e);
		}
	};

	eventSource.onopen = () => {
		console.log('EventSource connected');
		if (typeof document !== 'undefined') {
			document.body.setAttribute('data-sse-connected', 'true');
		}
	};

	eventSource.onerror = (e) => {
		console.error('EventSource error', e);
		if (typeof document !== 'undefined') {
			document.body.setAttribute('data-sse-connected', 'false');
		}
	};

	return () => {
		eventSource.close();
	};
}
