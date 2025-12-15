import { writable } from 'svelte/store';
import type { LogEntry } from '$lib/types';

const MAX_LOGS = 1000; // Keep last 1000 logs per service for mini-log
const MAX_STREAM_LOGS = 5000; // Keep last 5000 logs for the main stream

export const latestLog = writable<LogEntry | null>(null);

function createStreamStore() {
	const { subscribe, update, set } = writable<LogEntry[]>([]);

	return {
		subscribe,
		set,
		addLog: (entry: LogEntry) => {
			update((logs) => {
				const newLogs = [...logs, entry];
				if (newLogs.length > MAX_STREAM_LOGS) {
					newLogs.shift();
				}
				return newLogs;
			});
		},
		clear: () => set([])
	};
}

export const stream = createStreamStore();

function createLogsStore() {
	const { subscribe, update } = writable<Record<string, LogEntry[]>>({});

	return {
		subscribe,
		addLog: (entry: LogEntry) => {
			latestLog.set(entry);

			// Update per-service logs
			update((logs) => {
				const serviceLogs = logs[entry.service] || [];
				const newLogs = [...serviceLogs, entry];
				if (newLogs.length > MAX_LOGS) {
					newLogs.shift();
				}
				return {
					...logs,
					[entry.service]: newLogs
				};
			});

			// Update the unified stream
			stream.addLog(entry);
		}
	};
}

export const logs = createLogsStore();
