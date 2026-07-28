<script lang="ts">
	import { onMount } from 'svelte';
	import type { Terminal } from 'ghostty-web';
	import type { FitAddon } from 'ghostty-web';
	import { logIdentity, type LogEntry } from '$lib/types';
	import { logs, latestLog, stream } from '$lib/stores/logs';
	import { terminalTheme } from '$lib/theme';
	import { loadGhostty } from '$lib/ghostty';
	import { get } from 'svelte/store';

	let terminalContainer: HTMLDivElement;
	let terminal: Terminal;
	let fitAddon: FitAddon;

	let {
		filter = null,
		textFilter = ''
	}: { filter: string | string[] | null; textFilter?: string } = $props();

	function formatLog(entry: LogEntry, isMultiServiceFilter: boolean): string {
		let message = entry.message;

		// Always strip Clear Screen (2J) and Clear Scrollback (3J) to preserve history
		// eslint-disable-next-line no-control-regex
		message = message.replace(/\x1b\[[23]J/g, '');

		// If showing all services, strip cursor movement/clearing codes to prevent garbled output
		// Keep colors (m)
		const hasFilter = Array.isArray(filter) ? filter.length > 0 : !!filter;
		if (!hasFilter || isMultiServiceFilter) {
			// CSI sequences: ESC [ ... char
			// A-H: Cursor movement
			// J, K: Erase
			// S, T: Scroll
			// f: Horizontal/Vertical position
			// eslint-disable-next-line no-control-regex
			message = message.replace(/\x1b\[[\d;]*[A-HJKSTf]/g, '');
		}
		const d = new Date(entry.timestamp * 1000);
		const time = `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}:${String(d.getSeconds()).padStart(2, '0')}`;
		// Zinc-500: #71717a -> 113;113;122
		const timeColor = '\x1b[38;2;113;113;122m';
		if (hasFilter && !isMultiServiceFilter) {
			// Single-service view: skip our timestamp — the service's own output has one.
			return `${message}\r\n`;
		}
		// Strip project prefix (e.g. "dotlocal:dashboard" → "dashboard")
		const shortName = entry.service.includes(':') ? entry.service.split(':').pop()! : entry.service;
		// Zinc-300: #d4d4d8 -> 212;212;216
		const serviceColor = '\x1b[38;2;212;212;216m';
		return `${timeColor}${time}\x1b[0m ${serviceColor}${shortName}\x1b[0m ${message}\r\n`;
	}

	function isMultiServiceFilter(currentFilter: string | string[] | null): boolean {
		return Array.isArray(currentFilter) && currentFilter.length > 1;
	}

	function getFilteredLogs(currentFilter: string | string[] | null): LogEntry[] {
		if (!currentFilter || (Array.isArray(currentFilter) && currentFilter.length === 0)) {
			return get(stream);
		}

		if (Array.isArray(currentFilter)) {
			const merged = currentFilter.flatMap((service) => get(logs)[service] || []);
			return merged.sort((a, b) => a.timestamp - b.timestamp);
		}

		return get(logs)[currentFilter] || [];
	}

	function refresh(currentFilter: string | string[] | null, currentTextFilter: string) {
		if (!terminal) return;
		terminal.clear();

		const currentLogs = getFilteredLogs(currentFilter);
		const multiServiceFilter = isMultiServiceFilter(currentFilter);

		for (const entry of currentLogs) {
			if (
				!currentTextFilter ||
				entry.message.toLowerCase().includes(currentTextFilter.toLowerCase()) ||
				entry.service.toLowerCase().includes(currentTextFilter.toLowerCase())
			) {
				terminal.write(formatLog(entry, multiServiceFilter));
			}
		}
	}

	$effect(() => {
		// Re-render when filter or textFilter changes
		refresh(filter, textFilter);
	});

	onMount(() => {
		let cleanup: (() => void) | undefined;

		(async () => {
			const { Terminal, FitAddon } = await loadGhostty();

			terminal = new Terminal({
				cursorBlink: false,
				theme: terminalTheme,
				fontFamily: '"Geist Mono Variable", Menlo, Monaco, "Courier New", monospace',
				fontSize: 11,
				convertEol: true,
				disableStdin: true
			});

			fitAddon = new FitAddon();
			terminal.loadAddon(fitAddon);

			terminal.open(terminalContainer);
			fitAddon.fit();

			// Initial refresh
			refresh(filter, textFilter);

			// Subscribe to new logs
			const unsubscribeLogs = latestLog.subscribe((entry) => {
				if (entry && terminal) {
					const hasFilter = Array.isArray(filter) ? filter.length > 0 : !!filter;
					const matchesService =
						!hasFilter ||
						(Array.isArray(filter)
							? filter.includes(logIdentity(entry))
							: logIdentity(entry) === filter);
					const matchesText =
						!textFilter ||
						entry.message.toLowerCase().includes(textFilter.toLowerCase()) ||
						entry.service.toLowerCase().includes(textFilter.toLowerCase());

					if (matchesService && matchesText) {
						terminal.write(formatLog(entry, isMultiServiceFilter(filter)));
					}
				}
			});

			const resizeObserver = new ResizeObserver(() => {
				fitAddon.fit();
			});

			resizeObserver.observe(terminalContainer);

			cleanup = () => {
				unsubscribeLogs();
				resizeObserver.disconnect();
				terminal.dispose();
			};
		})();

		return () => {
			if (cleanup) cleanup();
		};
	});
</script>

<div class="terminal-container" bind:this={terminalContainer}></div>

<style>
	.terminal-container {
		width: 100%;
		height: 100%;
		background: #18181b; /* Match terminal theme background (zinc-900) */
		overflow: hidden;
	}
</style>
