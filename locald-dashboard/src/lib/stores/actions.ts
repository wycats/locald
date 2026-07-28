import { writable } from 'svelte/store';

export type ActionType = 'start' | 'stop' | 'restart' | 'reset';

interface PendingAction {
	serviceName: string;
	instanceId: string | null;
	action: ActionType;
}

function createActionsStore() {
	const { subscribe, update } = writable<PendingAction[]>([]);

	return {
		subscribe,
		start: (serviceName: string, instanceId: string | null, action: ActionType) => {
			update((actions) => [...actions, { serviceName, instanceId, action }]);
		},
		finish: (serviceName: string, instanceId: string | null, action: ActionType) => {
			update((actions) =>
				actions.filter(
					(a) =>
						!(a.serviceName === serviceName && a.instanceId === instanceId && a.action === action)
				)
			);
		}
	};
}

export const pendingActions = createActionsStore();
