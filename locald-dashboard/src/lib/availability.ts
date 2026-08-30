import type { ProjectAvailabilityStatus } from '$lib/api';

export function availabilityLabel(
	availability: ProjectAvailabilityStatus | null | undefined
): string {
	if (!availability) return 'Unknown';
	return availability.state
		.split('_')
		.map((part) => part[0].toUpperCase() + part.slice(1))
		.join(' ');
}

export function availabilityMessage(
	availability: ProjectAvailabilityStatus | null | undefined
): string {
	if (!availability) return 'Availability has not been reported by the daemon.';
	if (availability.last_error) return availability.last_error;
	if (availability.reasons.length > 0) return availability.reasons[0].message;
	if (availability.paused) return 'Paused until new meaningful activity resumes the project.';
	if (availability.desired) return 'locald is keeping this project available.';
	return 'No live demand or Always On policy currently requires this project.';
}

export function projectCanPause(
	availability: ProjectAvailabilityStatus | null | undefined
): boolean {
	return (
		availability?.desired === true &&
		!availability.paused &&
		(availability.state === 'ready' || availability.state === 'starting')
	);
}

export function projectCanResume(
	availability: ProjectAvailabilityStatus | null | undefined
): boolean {
	switch (availability?.state) {
		case 'paused':
		case 'stopped':
		case 'failed':
		case 'degraded':
		case 'cooling_down':
			return true;
		default:
			return false;
	}
}

export function demandSummary(
	availability: ProjectAvailabilityStatus | null | undefined
): string | null {
	if (!availability || availability.demands.length === 0) return null;
	const counts = new Map<string, number>();
	for (const demand of availability.demands) {
		counts.set(demand.safe_label, (counts.get(demand.safe_label) ?? 0) + 1);
	}
	return [...counts.entries()]
		.map(([label, count]) => (count === 1 ? label : `${count} ${label}`))
		.join(' · ');
}

export function formatTransition(
	timestamp: ProjectAvailabilityStatus['next_transition_at'] | null,
	now = Date.now()
): string | null {
	if (!timestamp) return null;
	const transition = timestamp.secs_since_epoch * 1000 + timestamp.nanos_since_epoch / 1_000_000;
	const remainingMilliseconds = Math.max(0, transition - now);
	if (remainingMilliseconds < 60_000) {
		return `in ${Math.ceil(remainingMilliseconds / 1_000)}s`;
	}
	if (remainingMilliseconds < 3_600_000) {
		return `in ${Math.ceil(remainingMilliseconds / 60_000)}m`;
	}
	return `in ${Math.ceil(remainingMilliseconds / 3_600_000)}h`;
}

export function transitionLabel(
	availability: ProjectAvailabilityStatus | null | undefined
): 'Next retry' | 'Next transition' {
	return availability?.desired && availability.last_error ? 'Next retry' : 'Next transition';
}
