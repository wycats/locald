import { describe, expect, it } from 'vitest';
import {
	availabilityLabel,
	availabilityMessage,
	demandSummary,
	formatTransition,
	transitionLabel,
	projectCanPause,
	projectCanResume
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

	it('offers resume only when convergence can make a current project more available', () => {
		for (const state of ['paused', 'stopped', 'failed', 'degraded', 'cooling_down'] as const) {
			expect(projectCanResume(status({ state }))).toBe(true);
		}
		expect(projectCanResume(status({ state: 'starting' }))).toBe(false);
		expect(projectCanResume(status({ state: 'ready' }))).toBe(false);
		expect(projectCanResume(status({ state: 'missing' }))).toBe(false);
		expect(projectCanResume(null)).toBe(false);
		expect(projectCanResume(undefined)).toBe(false);
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
		expect(formatTransition({ secs_since_epoch: 1_000, nanos_since_epoch: 0 }, 940_400)).toBe(
			'in 60s'
		);
	});

	it('labels desired-up convergence failures as retries across lifecycle states', () => {
		expect(transitionLabel(status({ state: 'failed', last_error: 'failed' }))).toBe('Next retry');
		expect(transitionLabel(status({ state: 'missing', last_error: 'missing' }))).toBe('Next retry');
		expect(
			transitionLabel(status({ state: 'cooling_down', desired: false, last_error: 'stopping' }))
		).toBe('Next transition');
		expect(transitionLabel(status({ state: 'cooling_down' }))).toBe('Next transition');
		expect(
			transitionLabel(
				status({
					state: 'failed',
					last_error: 'failed',
					demands: [
						{
							kind: 'vs_code_window',
							safe_label: 'VS Code window',
							expires_at: { secs_since_epoch: 1_000, nanos_since_epoch: 0 }
						}
					],
					next_transition_at: { secs_since_epoch: 1_000, nanos_since_epoch: 0 }
				})
			)
		).toBe('Next transition');
	});
});
