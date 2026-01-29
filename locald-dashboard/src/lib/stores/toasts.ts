import { writable } from 'svelte/store';

export type ToastType = 'success' | 'error' | 'info';

export interface Toast {
	id: string;
	type: ToastType;
	message: string;
	duration?: number;
}

function createToastStore() {
	const { subscribe, update } = writable<Toast[]>([]);

	const store = {
		subscribe,
		add: (type: ToastType, message: string, duration = 3000) => {
			const id = crypto.randomUUID();
			update((toasts) => [...toasts, { id, type, message, duration }]);
			if (duration > 0) {
				setTimeout(() => {
					update((toasts) => toasts.filter((t) => t.id !== id));
				}, duration);
			}
			return id;
		},
		remove: (id: string) => {
			update((toasts) => toasts.filter((t) => t.id !== id));
		},
		success: (message: string) => store.add('success', message),
		error: (message: string) => store.add('error', message, 5000)
	};

	return store;
}

export const toasts = createToastStore();
