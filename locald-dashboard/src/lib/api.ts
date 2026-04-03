import type { ServiceInspectResponse, ServiceStatus } from './types';
import { connection } from '$lib/stores/connection';

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

export async function resetService(name: string): Promise<void> {
	const res = await fetch(`/api/services/${name}/reset`, { method: 'POST' });
	if (!res.ok) {
		throw new Error(`Failed to reset service ${name}`);
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

export interface ProjectListEntry {
	project_path: string;
	project_name: string | null;
	attachments: Array<{
		source: { Editor?: { name: string; id: string }; CLI?: { pid: number }; Pin?: null };
	}>;
	is_running: boolean;
	section: 'Active' | 'AlwaysOn' | 'Recent';
}

export async function getProjects(): Promise<ProjectListEntry[]> {
	const res = await fetch('/api/projects');
	if (!res.ok) {
		throw new Error('Failed to fetch projects');
	}
	return res.json();
}

export async function removeProject(path: string): Promise<void> {
	const res = await fetch('/api/projects/remove', {
		method: 'POST',
		headers: { 'Content-Type': 'application/json' },
		body: JSON.stringify({ path })
	});
	if (!res.ok) {
		throw new Error('Failed to remove project');
	}
}

import { services } from '$lib/stores/services';
import { logs } from '$lib/stores/logs';

export async function getServiceInspect(name: string): Promise<ServiceInspectResponse> {
	const res = await fetch(`/api/services/${name}`);
	if (!res.ok) {
		throw new Error(`Failed to inspect service ${name}`);
	}
	return res.json();
}

let eventSource: EventSource | null = null;

function openEventSource() {
	connection.setConnecting();
	if (eventSource) {
		eventSource.close();
	}

	eventSource = new EventSource('/api/events');

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
		connection.setConnected();
		if (typeof document !== 'undefined') {
			document.body.setAttribute('data-sse-connected', 'true');
		}
	};

	eventSource.onerror = (e) => {
		console.error('EventSource error', e);
		connection.setDisconnected();
		if (typeof document !== 'undefined') {
			document.body.setAttribute('data-sse-connected', 'false');
		}
	};
}

export function connectEvents() {
	openEventSource();

	return () => {
		if (eventSource) {
			eventSource.close();
			eventSource = null;
		}
	};
}

export function reconnect() {
	openEventSource();
}
