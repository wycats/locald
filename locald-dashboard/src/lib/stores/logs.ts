import { derived, writable } from 'svelte/store';
import { logIdentity, type LogEntry } from '$lib/types';

const MAX_LOGS = 1000;
const MAX_STREAM_LOGS = 5000;

export interface LogHistory {
	recent: LogEntry[];
	live: LogEntry[];
}

interface LogState {
	stream: LogHistory;
	byService: Record<string, LogHistory>;
}

type LogListener = (entry: LogEntry) => void;
type StateChangeListener = () => void;

const emptyHistory = (): LogHistory => ({ recent: [], live: [] });
const state = writable<LogState>({ stream: emptyHistory(), byService: {} });
const liveListeners = new Set<LogListener>();
const stateChangeListeners = new Set<StateChangeListener>();
let replayBuffer: LogEntry[] | null = null;

function appendBounded(entries: LogEntry[], entry: LogEntry, capacity: number): LogEntry[] {
	const next = [...entries, entry];
	return next.length > capacity ? next.slice(next.length - capacity) : next;
}

function historyFrom(entries: LogEntry[], capacity: number): LogHistory {
	return { recent: entries.slice(-capacity), live: [] };
}

function appendLive(history: LogHistory, entry: LogEntry, capacity: number): LogHistory {
	let recent = history.recent;
	let live = appendBounded(history.live, entry, capacity);
	let overflow = recent.length + live.length - capacity;
	if (overflow > 0) {
		const recentEviction = Math.min(overflow, recent.length);
		recent = recent.slice(recentEviction);
		overflow -= recentEviction;
		if (overflow > 0) live = live.slice(overflow);
	}
	return { recent, live };
}

function withoutInstance(history: LogHistory, instanceId: string): LogHistory {
	return {
		recent: history.recent.filter((entry) => entry.instance_id !== instanceId),
		live: history.live.filter((entry) => entry.instance_id !== instanceId)
	};
}

function stateFromReplay(entries: LogEntry[]): LogState {
	const byServiceEntries: Record<string, LogEntry[]> = {};
	for (const entry of entries) {
		const identity = logIdentity(entry);
		byServiceEntries[identity] = appendBounded(byServiceEntries[identity] ?? [], entry, MAX_LOGS);
	}

	return {
		stream: historyFrom(entries, MAX_STREAM_LOGS),
		byService: Object.fromEntries(
			Object.entries(byServiceEntries).map(([identity, serviceEntries]) => [
				identity,
				historyFrom(serviceEntries, MAX_LOGS)
			])
		)
	};
}

export const stream = derived(state, ($state) => $state.stream);
const serviceLogs = derived(state, ($state) => $state.byService);

export const liveLogs = {
	subscribe(listener: LogListener) {
		liveListeners.add(listener);
		return () => liveListeners.delete(listener);
	}
};

export const logStateChanged = {
	subscribe(listener: StateChangeListener) {
		stateChangeListeners.add(listener);
		return () => stateChangeListeners.delete(listener);
	}
};

export const logs = {
	subscribe: serviceLogs.subscribe,
	beginReplay() {
		replayBuffer = [];
	},
	addLog(entry: LogEntry) {
		if (replayBuffer) {
			replayBuffer.push(entry);
			return;
		}

		state.update((current) => {
			const identity = logIdentity(entry);
			const serviceHistory = current.byService[identity] ?? emptyHistory();
			return {
				stream: appendLive(current.stream, entry, MAX_STREAM_LOGS),
				byService: {
					...current.byService,
					[identity]: appendLive(serviceHistory, entry, MAX_LOGS)
				}
			};
		});
		for (const listener of liveListeners) listener(entry);
	},
	finishReplay() {
		if (!replayBuffer) return;
		state.set(stateFromReplay(replayBuffer));
		replayBuffer = null;
		for (const listener of stateChangeListeners) listener();
	},
	retireInstance(instanceId: string) {
		state.update((current) => {
			const byService = Object.fromEntries(
				Object.entries(current.byService).flatMap(([identity, history]) => {
					const retained = withoutInstance(history, instanceId);
					return retained.recent.length + retained.live.length > 0 ? [[identity, retained]] : [];
				})
			);
			return {
				stream: withoutInstance(current.stream, instanceId),
				byService
			};
		});
		for (const listener of stateChangeListeners) listener();
	},
	clear() {
		replayBuffer = null;
		state.set({ stream: emptyHistory(), byService: {} });
	}
};
