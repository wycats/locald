import { describe, expect, it } from 'vitest';
import { formatLog, formatLogBoundary } from './log-format';
import type { LogEntry } from './types';

function entry(service: string, instanceId = 'instance', serviceDomain?: string): LogEntry {
	return {
		timestamp: 0,
		service,
		instance_id: instanceId,
		service_name: 'api:worker',
		service_domain: serviceDomain,
		stream: 'stdout',
		message: 'ready'
	};
}

describe('log formatting', () => {
	it('shows complete project-qualified identities in unified output', () => {
		expect(
			formatLog(entry('alpha:api:worker'), {
				hasFilter: false,
				isMultiServiceFilter: false
			})
		).toContain('alpha:api:worker');
	});

	it('distinguishes same-named services from different projects', () => {
		const alpha = formatLog(entry('alpha:workbench', 'instance-alpha', 'alpha.localhost'), {
			hasFilter: true,
			isMultiServiceFilter: true
		});
		const beta = formatLog(entry('beta:workbench', 'instance-beta', 'beta.localhost'), {
			hasFilter: true,
			isMultiServiceFilter: true
		});

		expect(alpha).toContain('alpha:workbench');
		expect(beta).toContain('beta:workbench');
		expect(alpha).not.toBe(beta);
	});

	it('distinguishes identical project services across worktree instances', () => {
		const first = formatLog(
			entry('app:workbench', 'instance-first', 'workbench.first.on.app.localhost'),
			{ hasFilter: true, isMultiServiceFilter: true }
		);
		const second = formatLog(
			entry('app:workbench', 'instance-second', 'workbench.second.on.app.localhost'),
			{ hasFilter: true, isMultiServiceFilter: true }
		);

		expect(first).toContain('app:workbench @ workbench.first.on.app.localhost');
		expect(second).toContain('app:workbench @ workbench.second.on.app.localhost');
		expect(first).not.toBe(second);
	});

	it('uses the full exact instance identity when no canonical domain exists', () => {
		const firstId = '00000000-0000-4000-8000-000000000001';
		const secondId = '00000000-0000-4000-8000-000000000002';
		const first = formatLog(entry('app:internal', firstId), {
			hasFilter: true,
			isMultiServiceFilter: true
		});
		const second = formatLog(entry('app:internal', secondId), {
			hasFilter: true,
			isMultiServiceFilter: true
		});

		expect(first).toContain(`app:internal @ ${firstId}`);
		expect(second).toContain(`app:internal @ ${secondId}`);
		expect(first).not.toBe(second);
	});

	it('renders explicit replay and live boundaries', () => {
		expect(formatLogBoundary('Recent history')).toContain('Recent history');
		expect(formatLogBoundary('Live')).toContain('Live');
	});
});
