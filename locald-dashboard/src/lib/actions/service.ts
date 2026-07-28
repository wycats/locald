import { startService, stopService, restartService, resetService } from '$lib/api';
import { pendingActions } from '$lib/stores/actions';
import { toasts } from '$lib/stores/toasts';

export async function startServiceWithFeedback(
	name: string,
	instanceId: string | null
): Promise<boolean> {
	pendingActions.start(name, instanceId, 'start');
	try {
		await startService(name, instanceId);
		toasts.success(`Started ${name}`);
		return true;
	} catch (e) {
		toasts.error(`Failed to start ${name}: ${e instanceof Error ? e.message : String(e)}`);
		return false;
	} finally {
		pendingActions.finish(name, instanceId, 'start');
	}
}

export async function stopServiceWithFeedback(
	name: string,
	instanceId: string | null
): Promise<boolean> {
	pendingActions.start(name, instanceId, 'stop');
	try {
		await stopService(name, instanceId);
		toasts.success(`Stopped ${name}`);
		return true;
	} catch (e) {
		toasts.error(`Failed to stop ${name}: ${e instanceof Error ? e.message : String(e)}`);
		return false;
	} finally {
		pendingActions.finish(name, instanceId, 'stop');
	}
}

export async function restartServiceWithFeedback(
	name: string,
	instanceId: string | null
): Promise<boolean> {
	pendingActions.start(name, instanceId, 'restart');
	try {
		await restartService(name, instanceId);
		toasts.success(`Restarted ${name}`);
		return true;
	} catch (e) {
		toasts.error(`Failed to restart ${name}: ${e instanceof Error ? e.message : String(e)}`);
		return false;
	} finally {
		pendingActions.finish(name, instanceId, 'restart');
	}
}

export async function resetServiceWithFeedback(
	name: string,
	instanceId: string | null
): Promise<boolean> {
	if (!confirm(`Reset "${name}"? This will stop the service and wipe all its data.`)) {
		return false;
	}

	pendingActions.start(name, instanceId, 'reset');
	try {
		await resetService(name, instanceId);
		toasts.success(`Reset ${name}`);
		return true;
	} catch (e) {
		toasts.error(`Failed to reset ${name}: ${e instanceof Error ? e.message : String(e)}`);
		return false;
	} finally {
		pendingActions.finish(name, instanceId, 'reset');
	}
}
