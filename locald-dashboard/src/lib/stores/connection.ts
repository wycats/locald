import { writable, derived } from 'svelte/store';

export type ConnectionState = 'connecting' | 'connected' | 'disconnected' | 'error';

interface ConnectionStatus {
	state: ConnectionState;
	errorMessage: string | null;
	lastConnected: Date | null;
	reconnectAttempts: number;
}

function createConnectionStore() {
	const { subscribe, set, update } = writable<ConnectionStatus>({
		state: 'connecting',
		errorMessage: null,
		lastConnected: null,
		reconnectAttempts: 0
	});

	return {
		subscribe,
		setConnected: () => {
			set({
				state: 'connected',
				errorMessage: null,
				lastConnected: new Date(),
				reconnectAttempts: 0
			});
		},
		setDisconnected: (errorMessage?: string) => {
			update((status) => ({
				...status,
				state: 'disconnected',
				errorMessage: errorMessage ?? 'Connection lost to server',
				reconnectAttempts: status.reconnectAttempts + 1
			}));
		},
		setError: (errorMessage: string) => {
			update((status) => ({
				...status,
				state: 'error',
				errorMessage
			}));
		},
		setConnecting: () => {
			update((status) => ({
				...status,
				state: 'connecting',
				errorMessage: null
			}));
		}
	};
}

export const connection = createConnectionStore();

export const isConnected = derived(connection, ($conn) => $conn.state === 'connected');
export const isDisconnected = derived(
	connection,
	($conn) => $conn.state === 'disconnected' || $conn.state === 'error'
);
