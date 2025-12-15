import { describe, it, expect } from 'vitest';
import { cleanLog } from './logs';

describe('cleanLog', () => {
	it('should return the message as is if no special characters', () => {
		expect(cleanLog('hello world')).toBe('hello world');
	});

	it('should strip clear screen ANSI codes', () => {
		// \x1b[2J is clear screen
		expect(cleanLog('hello\x1b[2J world')).toBe('hello world');
	});

	it('should strip clear scrollback ANSI codes', () => {
		// \x1b[3J is clear scrollback
		expect(cleanLog('hello\x1b[3J world')).toBe('hello world');
	});

	it('should handle carriage returns by taking the last segment', () => {
		expect(cleanLog('loading...\rloaded')).toBe('loaded');
		expect(cleanLog('step 1\rstep 2\rstep 3')).toBe('step 3');
	});

	it('should handle mixed ANSI and carriage returns', () => {
		// "downloading [==>  ]\rdownloading [====>]"
		expect(cleanLog('downloading [==>  ]\rdownloading [====>]')).toBe('downloading [====>]');
	});

	it('should handle multiple clear codes', () => {
		expect(cleanLog('\x1b[2J\x1b[3JClean')).toBe('Clean');
	});
});
