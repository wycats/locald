<script lang="ts">
	import { onMount } from 'svelte';
	import '@xterm/xterm/css/xterm.css';
	import type { Terminal } from '@xterm/xterm';
	import type { FitAddon } from '@xterm/addon-fit';
	import type { LogEntry } from '$lib/types';
	import { logs, latestLog, stream } from '$lib/stores/logs';
	import { terminalTheme } from '$lib/theme';
	import { get } from 'svelte/store';

	let terminalContainer: HTMLDivElement;
	let terminal: Terminal;
	let fitAddon: FitAddon;

	let { filter = null, textFilter = '' }: { filter: string | null; textFilter?: string } = $props();

	function formatLog(entry: LogEntry): string {
		let message = entry.message;

		// Always strip Clear Screen (2J) and Clear Scrollback (3J) to preserve history
		// eslint-disable-next-line no-control-regex
		message = message.replace(/\x1b\[[23]J/g, '');

		// If showing all services, strip cursor movement/clearing codes to prevent garbled output
		// Keep colors (m)
		if (!filter) {
			// CSI sequences: ESC [ ... char
			// A-H: Cursor movement
			// J, K: Erase
			// S, T: Scroll
			// f: Horizontal/Vertical position
			// eslint-disable-next-line no-control-regex
			message = message.replace(/\x1b\[[\d;]*[A-HJKSTf]/g, '');
		}
		const time = new Date(entry.timestamp * 1000).toLocaleTimeString();
		// Zinc-500: #71717a -> 113;113;122
		const timeColor = '\x1b[38;2;113;113;122m';
		if (filter) {
			return `${timeColor}${time}\x1b[0m ${message}\r\n`;
		}
		return `${timeColor}${time}\x1b[0m \x1b[1m${entry.service}\x1b[0m ${message}\r\n`;
	}

	function refresh(currentFilter: string | null, currentTextFilter: string) {
		if (!terminal) return;
		terminal.clear();

		let currentLogs: LogEntry[] = [];

		if (currentFilter) {
			// Service specific logs
			currentLogs = get(logs)[currentFilter] || [];
		} else {
			// All logs from the stream
			currentLogs = get(stream);
		}

		for (const entry of currentLogs) {
			if (
				!currentTextFilter ||
				entry.message.toLowerCase().includes(currentTextFilter.toLowerCase()) ||
				entry.service.toLowerCase().includes(currentTextFilter.toLowerCase())
			) {
				terminal.write(formatLog(entry));
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
			const { Terminal } = await import('@xterm/xterm');
			const { FitAddon } = await import('@xterm/addon-fit');

			terminal = new Terminal({
				cursorBlink: false,
				theme: terminalTheme,
				fontFamily: 'JetBrains Mono, Menlo, Monaco, "Courier New", monospace',
				fontSize: 13,
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
					const matchesService = !filter || entry.service === filter;
					const matchesText =
						!textFilter ||
						entry.message.toLowerCase().includes(textFilter.toLowerCase()) ||
						entry.service.toLowerCase().includes(textFilter.toLowerCase());

					if (matchesService && matchesText) {
						terminal.write(formatLog(entry));
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
		background: #09090b;
		overflow: hidden;
	}
</style>
