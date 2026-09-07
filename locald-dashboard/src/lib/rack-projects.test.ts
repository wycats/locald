import { describe, expect, it } from 'vitest';
import type { ProjectListEntry } from './api';
import type { ServiceStatus } from './types';
import { buildRackEntries, checkoutLabel, type ServiceProject } from './rack-projects';

const project = (path: string, name = 'shop'): ServiceProject => ({ name, path, services: [] });
const attached = (
	path: string,
	section: ProjectListEntry['section'] = 'Active'
): ProjectListEntry => ({
	project_name: 'shop',
	project_path: path,
	section,
	attachments: [],
	is_running: true
});

describe('checkout labels', () => {
	it('uses the shortest distinguishing suffix for repeated checkout basenames', () => {
		const paths = ['/work/feature-a/shop', '/work/feature-b/shop', '/work/other'];
		expect(checkoutLabel(paths[0], paths)).toBe('feature-a/shop');
		expect(checkoutLabel(paths[1], paths)).toBe('feature-b/shop');
		expect(checkoutLabel(paths[2], paths)).toBe('other');
	});

	it('handles repeated parent basenames and trailing separators', () => {
		const paths = ['/first/checkouts/shop/', '/second/checkouts/shop/'];
		expect(checkoutLabel(paths[0], paths)).toBe('first/checkouts/shop');
		expect(checkoutLabel(paths[1], paths)).toBe('second/checkouts/shop');
	});
});

describe('rack project identity', () => {
	it('preserves unlisted same-named worktrees with their actual paths', () => {
		const first = project('/work/feature-a/shop');
		const second = project('/work/feature-b/shop');
		const entries = buildRackEntries([attached(first.path!)], [], [], [first, second], []);
		expect(entries.filter((entry) => entry.kind === 'project')).toEqual([
			{
				...first,
				kind: 'project',
				key: first.path,
				entry: attached(first.path!),
				section: 'Active',
				checkoutLabel: 'feature-a/shop'
			},
			{
				...second,
				kind: 'project',
				key: second.path,
				entry: null,
				section: null,
				checkoutLabel: 'feature-b/shop'
			}
		]);
	});

	it('collapsing a section claims only its actual checkout', () => {
		const first = project('/work/feature-a/shop');
		const second = project('/work/feature-b/shop');
		const entries = buildRackEntries([attached(first.path!)], [], [], [first, second], ['Active']);
		expect(entries.filter((entry) => entry.kind === 'project').map((entry) => entry.key)).toEqual([
			second.path
		]);
	});

	it('disambiguates names across sections even while a section is collapsed', () => {
		const entries = buildRackEntries(
			[attached('/work/a/shop')],
			[],
			[attached('/work/b/shop', 'Recent')],
			[],
			['Recent']
		);
		expect(entries.find((entry) => entry.kind === 'project')).toMatchObject({
			checkoutLabel: 'a/shop'
		});
	});

	it('keeps pathless instances independent and avoids ambiguous legacy attachment', () => {
		const first = {
			name: 'shop',
			path: null,
			services: [{ instance_id: 'first' } as ServiceStatus]
		};
		const second = {
			name: 'shop',
			path: null,
			services: [{ instance_id: 'second' } as ServiceStatus]
		};
		const entries = buildRackEntries([attached('/work/a/shop')], [], [], [first, second], []);
		expect(entries.filter((entry) => entry.kind === 'project').map((entry) => entry.key)).toEqual([
			'/work/a/shop',
			'first',
			'second'
		]);
	});

	it('omits the checkout qualifier when a project name already identifies one path', () => {
		const entries = buildRackEntries([attached('/work/shop')], [], [], [project('/work/shop')], []);
		expect(entries.find((entry) => entry.kind === 'project')).toMatchObject({
			checkoutLabel: null
		});
	});
});
