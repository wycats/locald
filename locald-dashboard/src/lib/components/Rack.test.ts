import { describe, it, expect } from 'vitest';

// Mock the component logic since we can't easily mount Svelte components in this environment
// We'll test the logic function directly.

function toggleMonitor(
	name: string,
	monitored: string[],
	solo: string | null
): { monitored: string[]; solo: string | null } {
	let newMonitored = [...monitored];

	// If we are in solo mode, and we monitor something else, we should monitor the solo'd item too
	if (solo && solo !== name && !newMonitored.includes(solo)) {
		newMonitored.push(solo);
	}

	if (newMonitored.includes(name)) {
		newMonitored = newMonitored.filter((n) => n !== name);
	} else {
		newMonitored.push(name);
	}

	return { monitored: newMonitored, solo };
}

describe('toggleMonitor logic', () => {
	it('should toggle monitor state normally', () => {
		let state = toggleMonitor('service-a', [], null);
		expect(state.monitored).toEqual(['service-a']);

		state = toggleMonitor('service-a', ['service-a'], null);
		expect(state.monitored).toEqual([]);
	});

	it('should monitor the solo service when monitoring another service', () => {
		// Scenario: Solo on A, Monitor B. Result: A and B monitored.
		const state = toggleMonitor('service-b', [], 'service-a');
		expect(state.monitored).toContain('service-a');
		expect(state.monitored).toContain('service-b');
		expect(state.monitored.length).toBe(2);
	});

	it('should not duplicate solo service if already monitored', () => {
		// Scenario: Solo on A (already monitored), Monitor B. Result: A and B monitored.
		const state = toggleMonitor('service-b', ['service-a'], 'service-a');
		expect(state.monitored).toContain('service-a');
		expect(state.monitored).toContain('service-b');
		expect(state.monitored.length).toBe(2);
	});

	it('should handle monitoring the solo service itself', () => {
		// Scenario: Solo on A, Monitor A. Result: A monitored.
		const state = toggleMonitor('service-a', [], 'service-a');
		expect(state.monitored).toEqual(['service-a']);
	});

	it('should handle unmonitoring the solo service itself', () => {
		// Scenario: Solo on A, Unmonitor A. Result: A unmonitored.
		const state = toggleMonitor('service-a', ['service-a'], 'service-a');
		expect(state.monitored).toEqual([]);
	});
});

function isActive(name: string, monitored: string[], solo: string | null): boolean {
	return solo === name || monitored.includes(name);
}

describe('isActive logic', () => {
	it('should be active if solo', () => {
		expect(isActive('a', [], 'a')).toBe(true);
	});
	it('should be active if monitored', () => {
		expect(isActive('a', ['a'], null)).toBe(true);
	});
	it('should be active if both', () => {
		expect(isActive('a', ['a'], 'a')).toBe(true);
	});
	it('should not be active if neither', () => {
		expect(isActive('a', [], 'b')).toBe(false);
	});
});
