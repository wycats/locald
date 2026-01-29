import { startService, stopService, restartService, resetService } from '$lib/api';
import { pendingActions } from '$lib/stores/actions';
import { toasts } from '$lib/stores/toasts';

export async function startServiceWithFeedback(name: string): Promise<boolean> {
	pendingActions.start(name, 'start');
	try {
		await startService(name);
		toasts.success(`Started ${name}`);
		return true;
	} catch (e) {
		toasts.error(`Failed to start ${name}: ${e instanceof Error ? e.message : String(e)}`);
		return false;
	} finally {
		pendingActions.finish(name, 'start');
	}
}

export async function stopServiceWithFeedback(name: string): Promise<boolean> {
	pendingActions.start(name, 'stop');
	try {
		await stopService(name);
		toasts.success(`Stopped ${name}`);
		return true;
	} catch (e) {
		toasts.error(`Failed to stop ${name}: ${e instanceof Error ? e.message : String(e)}`);
		return false;
	} finally {
		pendingActions.finish(name, 'stop');
	}
}

export async function restartServiceWithFeedback(name: string): Promise<boolean> {
	pendingActions.start(name, 'restart');
	try {
		await restartService(name);
		toasts.success(`Restarted ${name}`);
		return true;
	} catch (e) {
		toasts.error(`Failed to restart ${name}: ${e instanceof Error ? e.message : String(e)}`);
		return false;
	} finally {
		pendingActions.finish(name, 'restart');
	}
}

export async function resetServiceWithFeedback(name: string): Promise<boolean> {
	if (!confirm(`Reset "${name}"? This will stop the service and wipe all its data.`)) {
		return false;
	}

	pendingActions.start(name, 'reset');
	try {
		await resetService(name);
		toasts.success(`Reset ${name}`);
		return true;
	} catch (e) {
		toasts.error(`Failed to reset ${name}: ${e instanceof Error ? e.message : String(e)}`);
		return false;
	} finally {
		pendingActions.finish(name, 'reset');
	}
}
