import { describe, expect, it } from 'vitest';
import { displayServiceEndpoint } from './service-display';

describe('displayServiceEndpoint', () => {
	it('prefers public service URLs over connection URLs', () => {
		expect(
			displayServiceEndpoint({
				domain: 'api.example.localhost',
				url: 'https://api.example.localhost',
				connection_url: 'http://localhost:3000'
			})
		).toEqual({
			kind: 'public',
			label: 'api.example.localhost',
			value: 'https://api.example.localhost'
		});
	});

	it('falls back to connection URLs when no public URL is available', () => {
		expect(
			displayServiceEndpoint({
				domain: null,
				url: null,
				connection_url: 'postgres://postgres@localhost:15432/postgres'
			})
		).toEqual({
			kind: 'connection',
			label: 'postgres://postgres@localhost:15432/postgres',
			value: 'postgres://postgres@localhost:15432/postgres'
		});
	});

	it('returns null when neither endpoint is available', () => {
		expect(
			displayServiceEndpoint({
				domain: null,
				url: null,
				connection_url: null
			})
		).toBeNull();
	});
});
