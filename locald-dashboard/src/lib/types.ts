export type ServiceType = 'exec' | 'postgres' | 'worker' | 'container' | 'site';

export interface ServiceStatus {
	name: string;
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
	cpu_percent: number;
	memory_bytes: number;
	timestamp: number;
}

export interface LogEntry {
	timestamp: number;
	service: string;
	stream: string;
	message: string;
}
