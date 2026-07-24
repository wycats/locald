import { describe, expect, it } from 'vitest';
import {
	availabilityLabel,
	availabilityMessage,
	demandSummary,
	formatTransition,
	projectCanPause
} from './availability';
import type { ProjectAvailabilityStatus } from './api';

function status(overrides: Partial<ProjectAvailabilityStatus> = {}): ProjectAvailabilityStatus {
	return {
		desired: true,
		state: 'ready',
		always_on: false,
		paused: false,
		reasons: [],
		demands: [],
		...overrides
	};
}

describe('project availability presentation', () => {
	it('uses daemon lifecycle state and reasons', () => {
		const availability = status({
			state: 'cooling_down',
			desired: false,
			reasons: [{ code: 'cooldown', message: 'Stopping after the cooldown.' }]
		});
		expect(availabilityLabel(availability)).toBe('Cooling Down');
		expect(availabilityMessage(availability)).toBe('Stopping after the cooldown.');
		expect(projectCanPause(availability)).toBe(false);
	});

	it('makes an error the controlling explanation', () => {
		const availability = status({
			state: 'failed',
			last_error: 'Health check timed out.',
			reasons: [{ code: 'desired_up', message: 'A demand remains live.' }]
		});
		expect(availabilityMessage(availability)).toBe('Health check timed out.');
		expect(projectCanPause(availability)).toBe(false);
	});

	it('offers convergence rather than pause for a degraded desired project', () => {
		expect(projectCanPause(status({ state: 'degraded' }))).toBe(false);
		expect(projectCanPause(status({ state: 'ready' }))).toBe(true);
	});

	it('summarizes privacy-safe demand labels without owner identifiers', () => {
		const availability = status({
			demands: [
				{ kind: 'vs_code_window', safe_label: 'VS Code window' },
				{ kind: 'vs_code_window', safe_label: 'VS Code window' },
				{ kind: 'manual_cli', safe_label: 'Manual CLI' }
			]
		});
		expect(demandSummary(availability)).toBe('2 VS Code window · Manual CLI');
	});

	it('formats absolute persisted transition timestamps relative to now', () => {
		expect(formatTransition({ secs_since_epoch: 1_000, nanos_since_epoch: 0 }, 940_000)).toBe(
			'in 1m'
		);
	});
});
