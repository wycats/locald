import { get } from 'svelte/store';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { liveLogs, logs, logStateChanged, stream } from './logs';
import type { LogEntry } from '$lib/types';

function entry(
	message: string,
	options: Partial<
		Pick<LogEntry, 'service' | 'instance_id' | 'service_name' | 'service_domain' | 'timestamp'>
	> = {}
): LogEntry {
	return {
		timestamp: options.timestamp ?? 1,
		service: options.service ?? 'alpha:workbench',
		instance_id: options.instance_id ?? 'instance-alpha',
		service_name: options.service_name ?? 'workbench',
		service_domain: options.service_domain,
		stream: 'stdout',
		message
	};
}

afterEach(() => logs.clear());

describe('lifecycle-aware log stores', () => {
	it('commits a replay atomically and preserves payload text exactly', () => {
		logs.addLog(entry('previous live line'));
		logs.beginReplay();
		logs.addLog(entry('line one\nline two'));

		expect(get(stream).live.map((log) => log.message)).toEqual(['previous live line']);

		logs.finishReplay();

		expect(get(stream)).toEqual({
			recent: [entry('line one\nline two')],
			live: []
		});
	});

	it('replaces reconnect snapshots instead of duplicating them', () => {
		const first = entry('first', { timestamp: 1 });
		const second = entry('second', { timestamp: 2 });
		logs.beginReplay();
		logs.addLog(first);
		logs.finishReplay();
		logs.addLog(second);

		logs.beginReplay();
		logs.addLog(first);
		logs.addLog(second);
		logs.finishReplay();

		expect(get(stream)).toEqual({ recent: [first, second], live: [] });
	});

	it('keys colon-containing service names by instance and configured name', () => {
		const first = entry('first', {
			service: 'alpha:api:worker',
			instance_id: 'instance-alpha',
			service_name: 'api:worker'
		});
		const second = entry('second', {
			service: 'beta:api:worker',
			instance_id: 'instance-beta',
			service_name: 'api:worker'
		});
		logs.beginReplay();
		logs.addLog(first);
		logs.addLog(second);
		logs.finishReplay();

		const byService = get(logs);
		expect(byService['instance-alpha/api:worker']?.recent).toEqual([first]);
		expect(byService['instance-beta/api:worker']?.recent).toEqual([second]);
	});

	it('notifies subscribers only for new live entries', () => {
		logs.addLog(entry('before subscription'));
		const listener = vi.fn();
		const unsubscribe = liveLogs.subscribe(listener);

		logs.addLog(entry('after subscription'));

		expect(listener).toHaveBeenCalledOnce();
		expect(listener).toHaveBeenCalledWith(entry('after subscription'));
		unsubscribe();
	});

	it('notifies terminals only after the replay snapshot commits', () => {
		const listener = vi.fn();
		const unsubscribe = logStateChanged.subscribe(listener);
		logs.beginReplay();
		logs.addLog(entry('recent'));
		expect(listener).not.toHaveBeenCalled();

		logs.finishReplay();

		expect(listener).toHaveBeenCalledOnce();
		unsubscribe();
	});

	it('retires only one exact instance across recent and live history', () => {
		const firstRecent = entry('first recent', { instance_id: 'instance-first' });
		const secondRecent = entry('second recent', { instance_id: 'instance-second' });
		logs.beginReplay();
		logs.addLog(firstRecent);
		logs.addLog(secondRecent);
		logs.finishReplay();
		logs.addLog(entry('first live', { instance_id: 'instance-first' }));
		const secondLive = entry('second live', { instance_id: 'instance-second' });
		logs.addLog(secondLive);
		const listener = vi.fn();
		const unsubscribe = logStateChanged.subscribe(listener);

		logs.retireInstance('instance-first');

		expect(get(stream)).toEqual({ recent: [secondRecent], live: [secondLive] });
		expect(Object.keys(get(logs))).toEqual(['instance-second/workbench']);
		expect(listener).toHaveBeenCalledOnce();
		unsubscribe();
	});

	it('recovers a missed retirement when the next replay replaces history', () => {
		const retained = entry('retained', { instance_id: 'instance-second' });
		logs.beginReplay();
		logs.addLog(entry('retired', { instance_id: 'instance-first' }));
		logs.addLog(retained);
		logs.finishReplay();

		logs.beginReplay();
		logs.addLog(retained);
		logs.finishReplay();

		expect(get(stream)).toEqual({ recent: [retained], live: [] });
		expect(Object.keys(get(logs))).toEqual(['instance-second/workbench']);
	});

	it('keeps recent and live entries within the original total stream cap', () => {
		logs.beginReplay();
		for (let index = 0; index < 5000; index++) {
			logs.addLog(entry(`recent ${index}`, { timestamp: index }));
		}
		logs.finishReplay();

		logs.addLog(entry('live', { timestamp: 5000 }));

		const history = get(stream);
		expect(history.recent).toHaveLength(4999);
		expect(history.recent[0]?.message).toBe('recent 1');
		expect(history.live.map((item) => item.message)).toEqual(['live']);
		expect(history.recent.length + history.live.length).toBe(5000);
	});
});
