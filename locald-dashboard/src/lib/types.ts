export type ServiceType = 'exec' | 'postgres' | 'worker' | 'container' | 'site';

export interface ServiceStatus {
	name: string;
	instance_id: string | null;
	service_name: string | null;
	service_type: ServiceType;
	pid: number | null;
	port: number | null;
	status: 'running' | 'stopped' | 'building';
	url: string | null;
	connection_url: string | null;
	domain: string | null;
	health_status: string;
	health_source: string;
	path: string | null;
	workspace: string | null;
	constellation: string | null;
	warnings: string[];
	metrics?: ServiceMetrics;
	cpu_history?: number[];
}

export interface ServiceMetrics {
	name: string;
	instance_id: string | null;
	service_name: string | null;
	cpu_percent: number;
	memory_bytes: number;
	timestamp: number;
}

export interface LogEntry {
	timestamp: number;
	service: string;
	instance_id?: string | null;
	service_name?: string | null;
	stream: string;
	message: string;
}

export interface ServiceInspectResponse {
	name: string;
	instance_id: string;
	pid: number | null;
	port: number | null;
	url: string | null;
	connection_url?: string;
	health_status: string;
	health_source: string;
	path: string | null;
	container_id: string | null;
	warnings: string[];
	config?: unknown;
}

export function serviceIdentity(
	service: Pick<ServiceStatus, 'instance_id' | 'name'> &
		Partial<Pick<ServiceStatus, 'service_name'>>
): string {
	return service.instance_id
		? `${service.instance_id}/${service.service_name ?? service.name}`
		: service.name;
}

export function resolveServiceSelector(selector: string, candidates: ServiceStatus[]): string {
	if (
		selector === 'locald' ||
		candidates.some((service) => serviceIdentity(service) === selector)
	) {
		return selector;
	}
	const legacyMatches = candidates.filter((service) => service.name === selector);
	return legacyMatches.length === 1 ? serviceIdentity(legacyMatches[0]) : selector;
}

export function logIdentity(
	entry: Pick<LogEntry, 'instance_id' | 'service'> & Partial<Pick<LogEntry, 'service_name'>>
): string {
	return entry.instance_id
		? `${entry.instance_id}/${entry.service_name ?? entry.service}`
		: entry.service;
}
