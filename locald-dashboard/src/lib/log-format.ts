import type { LogEntry } from '$lib/types';

export function formatLog(
	entry: LogEntry,
	options: { hasFilter: boolean; isMultiServiceFilter: boolean }
): string {
	let message = entry.message;

	// Preserve history by ignoring process attempts to clear the terminal.
	// eslint-disable-next-line no-control-regex
	message = message.replace(/\x1b\[[23]J/g, '');

	if (!options.hasFilter || options.isMultiServiceFilter) {
		// Keep color sequences while removing cursor movement and erasure that
		// would corrupt interleaved output from multiple services.
		// eslint-disable-next-line no-control-regex
		message = message.replace(/\x1b\[[\d;]*[A-HJKSTf]/g, '');
	}

	if (options.hasFilter && !options.isMultiServiceFilter) {
		return `${message}\r\n`;
	}

	const date = new Date(entry.timestamp * 1000);
	const time = `${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(2, '0')}:${String(date.getSeconds()).padStart(2, '0')}`;
	const timeColor = '\x1b[38;2;113;113;122m';
	const serviceColor = '\x1b[38;2;212;212;216m';
	const instanceQualifier = entry.service_domain ?? entry.instance_id;
	const source = instanceQualifier ? `${entry.service} @ ${instanceQualifier}` : entry.service;
	return `${timeColor}${time}\x1b[0m ${serviceColor}${source}\x1b[0m ${message}\r\n`;
}

export function formatLogBoundary(label: 'Recent history' | 'Live'): string {
	return `\x1b[38;2;113;113;122m── ${label} ──\x1b[0m\r\n`;
}
