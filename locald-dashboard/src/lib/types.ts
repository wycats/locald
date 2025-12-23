export interface ServiceStatus {
	name: string;
	pid: number | null;
	port: number | null;
	status: 'running' | 'stopped' | 'building';
	url: string | null;
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
