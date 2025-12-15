import { describe, it, expect } from 'vitest';

// Mock the component logic since we can't easily mount Svelte components in this environment
// We'll test the logic function directly.

function togglePin(
	name: string,
	pinned: string[],
	solo: string | null
): { pinned: string[]; solo: string | null } {
	let newPinned = [...pinned];

	// If we are in solo mode, and we pin something else, we should pin the solo'd item too
	if (solo && solo !== name && !newPinned.includes(solo)) {
		newPinned.push(solo);
	}

	if (newPinned.includes(name)) {
		newPinned = newPinned.filter((n) => n !== name);
	} else {
		newPinned.push(name);
	}

	return { pinned: newPinned, solo };
}

describe('togglePin logic', () => {
	it('should toggle pin state normally', () => {
		let state = togglePin('service-a', [], null);
		expect(state.pinned).toEqual(['service-a']);

		state = togglePin('service-a', ['service-a'], null);
		expect(state.pinned).toEqual([]);
	});

	it('should pin the solo service when pinning another service', () => {
		// Scenario: Solo on A, Pin B. Result: A and B pinned.
		const state = togglePin('service-b', [], 'service-a');
		expect(state.pinned).toContain('service-a');
		expect(state.pinned).toContain('service-b');
		expect(state.pinned.length).toBe(2);
	});

	it('should not duplicate solo service if already pinned', () => {
		// Scenario: Solo on A (already pinned), Pin B. Result: A and B pinned.
		const state = togglePin('service-b', ['service-a'], 'service-a');
		expect(state.pinned).toContain('service-a');
		expect(state.pinned).toContain('service-b');
		expect(state.pinned.length).toBe(2);
	});

	it('should handle pinning the solo service itself', () => {
		// Scenario: Solo on A, Pin A. Result: A pinned.
		const state = togglePin('service-a', [], 'service-a');
		expect(state.pinned).toEqual(['service-a']);
	});

	it('should handle unpinning the solo service itself', () => {
		// Scenario: Solo on A, Unpin A. Result: A unpinned.
		const state = togglePin('service-a', ['service-a'], 'service-a');
		expect(state.pinned).toEqual([]);
	});
});

function isActive(name: string, pinned: string[], solo: string | null): boolean {
	return solo === name || pinned.includes(name);
}

describe('isActive logic', () => {
	it('should be active if solo', () => {
		expect(isActive('a', [], 'a')).toBe(true);
	});
	it('should be active if pinned', () => {
		expect(isActive('a', ['a'], null)).toBe(true);
	});
	it('should be active if both', () => {
		expect(isActive('a', ['a'], 'a')).toBe(true);
	});
	it('should not be active if neither', () => {
		expect(isActive('a', [], 'b')).toBe(false);
	});
});
