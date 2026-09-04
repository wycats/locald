import type { ServiceInspectResponse, ServiceStatus } from './types';
import { connection } from '$lib/stores/connection';

export async function getServices(): Promise<ServiceStatus[]> {
	const res = await fetch('/api/state');
	if (!res.ok) {
		throw new Error('Failed to fetch services');
	}
	return res.json();
}

function serviceApiPath(name: string, instanceId: string | null): string {
	const encodedName = encodeURIComponent(name);
	return instanceId
		? `/api/instances/${encodeURIComponent(instanceId)}/services/${encodedName}`
		: `/api/services/${encodedName}`;
}

export async function startService(name: string, instanceId: string | null): Promise<void> {
	const res = await fetch(`${serviceApiPath(name, instanceId)}/start`, { method: 'POST' });
	if (!res.ok) {
		throw new Error(`Failed to start service ${name}`);
	}
}

export async function stopService(name: string, instanceId: string | null): Promise<void> {
	const res = await fetch(`${serviceApiPath(name, instanceId)}/stop`, { method: 'POST' });
	if (!res.ok) {
		throw new Error(`Failed to stop service ${name}`);
	}
}

export async function restartService(name: string, instanceId: string | null): Promise<void> {
	const res = await fetch(`${serviceApiPath(name, instanceId)}/restart`, { method: 'POST' });
	if (!res.ok) {
		throw new Error(`Failed to restart service ${name}`);
	}
}

export async function resetService(name: string, instanceId: string | null): Promise<void> {
	const res = await fetch(`${serviceApiPath(name, instanceId)}/reset`, { method: 'POST' });
	if (!res.ok) {
		throw new Error(`Failed to reset service ${name}`);
	}
}

export async function stopAllServices(): Promise<void> {
	const res = await fetch('/api/services/stop-all', { method: 'POST' });
	if (!res.ok) {
		throw new Error('Failed to stop all managed services');
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
	availability?: ProjectAvailabilityStatus;
}

export type DemandKind =
	| 'manual_cli'
	| 'vs_code_window'
	| 'agent_conversation'
	| 'legacy_process_attachment'
	| 'stopped_page_resume';

export type ProjectLifecycleState =
	| 'starting'
	| 'ready'
	| 'degraded'
	| 'failed'
	| 'cooling_down'
	| 'paused'
	| 'stopped'
	| 'missing';

export interface SystemTimestamp {
	secs_since_epoch: number;
	nanos_since_epoch: number;
}

export interface ProjectAvailabilityStatus {
	desired: boolean;
	state: ProjectLifecycleState;
	always_on: boolean;
	paused: boolean;
	reasons: Array<{ code: string; message: string }>;
	demands: Array<{
		kind: DemandKind;
		safe_label: string;
		expires_at?: SystemTimestamp;
	}>;
	next_transition_at?: SystemTimestamp;
	last_error?: string;
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

async function postProjectAction(
	endpoint: string,
	body: Record<string, string | boolean>
): Promise<Response> {
	const res = await fetch(`/api/projects/${endpoint}`, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json' },
		body: JSON.stringify(body)
	});
	if (!res.ok) {
		const detail = await res.text();
		throw new Error(detail || `Project ${endpoint} failed`);
	}
	return res;
}

export async function resumeProject(path: string): Promise<void> {
	await postProjectAction('resume', { path });
}

export async function pauseProject(path: string): Promise<void> {
	await postProjectAction('pause', { path });
}

export async function setProjectAlwaysOn(path: string, enabled: boolean): Promise<void> {
	await postProjectAction('always-on', { path, enabled });
}

import { services } from '$lib/stores/services';
import { logs } from '$lib/stores/logs';

export async function getServiceInspect(
	name: string,
	instanceId: string | null
): Promise<ServiceInspectResponse> {
	const res = await fetch(serviceApiPath(name, instanceId));
	if (!res.ok) {
		throw new Error(`Failed to inspect service ${name}`);
	}
	return res.json();
}

let eventSource: EventSource | null = null;
let lifecycleChangeCallback: (() => void) | undefined;

function openEventSource() {
	connection.setConnecting();
	if (eventSource) {
		eventSource.close();
	}

	eventSource = new EventSource('/api/events');

	eventSource.onmessage = (event) => {
		try {
			const msg = JSON.parse(event.data);
			if (msg.type === 'LogReplayStarted') {
				logs.beginReplay();
			} else if (msg.type === 'Log') {
				logs.addLog(msg.data);
			} else if (msg.type === 'LogReplayFinished') {
				logs.finishReplay();
			} else if (msg.type === 'LogInstanceRetired') {
				logs.retireInstance(msg.data);
			} else if (msg.type === 'ServiceUpdate') {
				console.log('Received ServiceUpdate:', msg.data.name, msg.data.status);
				services.updateService(msg.data);
				lifecycleChangeCallback?.();
			} else if (msg.type === 'ServiceListChanged') {
				void services.refresh();
				lifecycleChangeCallback?.();
			}
		} catch (e) {
			console.error('Failed to parse event', e);
		}
	};

	eventSource.onopen = () => {
		console.log('EventSource connected');
		resetReconnectDelay();
		connection.setConnected();
		void services.refresh();
		lifecycleChangeCallback?.();
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
		// Auto-reconnect with backoff. EventSource's native retry gives up
		// when the server is down; we retry manually on a longer interval.
		if (eventSource?.readyState === EventSource.CLOSED) {
			scheduleReconnect();
		}
	};
}

let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
let reconnectDelay = 2000;
const MAX_RECONNECT_DELAY = 30000;

function scheduleReconnect() {
	if (reconnectTimer) return;
	reconnectTimer = setTimeout(() => {
		reconnectTimer = null;
		console.log(`Reconnecting SSE (after ${reconnectDelay}ms)...`);
		openEventSource();
		// Back off, but cap at 30s
		reconnectDelay = Math.min(reconnectDelay * 1.5, MAX_RECONNECT_DELAY);
	}, reconnectDelay);
}

function resetReconnectDelay() {
	reconnectDelay = 2000;
	if (reconnectTimer) {
		clearTimeout(reconnectTimer);
		reconnectTimer = null;
	}
}

export function connectEvents(onLifecycleChange?: () => void) {
	lifecycleChangeCallback = onLifecycleChange;
	openEventSource();

	return () => {
		resetReconnectDelay();
		if (lifecycleChangeCallback === onLifecycleChange) {
			lifecycleChangeCallback = undefined;
		}
		if (eventSource) {
			eventSource.close();
			eventSource = null;
		}
	};
}

export function reconnect() {
	resetReconnectDelay();
	openEventSource();
}
