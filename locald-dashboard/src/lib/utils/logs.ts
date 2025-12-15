export function cleanLog(message: string): string {
	// Strip CSI sequences (cursor movement, clear line, etc)
	// eslint-disable-next-line no-control-regex
	const stripped = message.replace(/\x1b\[[\d;]*[A-HJKSTf]/g, '');
	// Handle carriage return by taking the last segment
	const parts = stripped.split('\r');
	return parts[parts.length - 1];
}
