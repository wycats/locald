import { writable } from 'svelte/store';

export type ActionType = 'start' | 'stop' | 'restart' | 'reset';

interface PendingAction {
	serviceName: string;
	action: ActionType;
}

function createActionsStore() {
	const { subscribe, update } = writable<PendingAction[]>([]);

	return {
		subscribe,
		start: (serviceName: string, action: ActionType) => {
			update((actions) => [...actions, { serviceName, action }]);
		},
		finish: (serviceName: string, action: ActionType) => {
			update((actions) =>
				actions.filter((a) => !(a.serviceName === serviceName && a.action === action))
			);
		}
	};
}

export const pendingActions = createActionsStore();
