<script lang="ts">
	import { onMount } from 'svelte';
	import type { Terminal } from 'ghostty-web';
	import type { FitAddon } from 'ghostty-web';
	import { logIdentity, type LogEntry } from '$lib/types';
	import { logs, liveLogs, logStateChanged, stream, type LogHistory } from '$lib/stores/logs';
	import { formatLog, formatLogBoundary } from '$lib/log-format';
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

	function isMultiServiceFilter(currentFilter: string | string[] | null): boolean {
		return Array.isArray(currentFilter) && currentFilter.length > 1;
	}

	function getFilteredLogs(currentFilter: string | string[] | null): LogHistory {
		if (!currentFilter || (Array.isArray(currentFilter) && currentFilter.length === 0)) {
			return get(stream);
		}

		if (Array.isArray(currentFilter)) {
			const selected = currentFilter.map((service) => get(logs)[service]);
			return {
				recent: selected.flatMap((history) => history?.recent ?? []).sort(byTimestamp),
				live: selected.flatMap((history) => history?.live ?? []).sort(byTimestamp)
			};
		}

		return get(logs)[currentFilter] ?? { recent: [], live: [] };
	}

	function byTimestamp(a: LogEntry, b: LogEntry): number {
		return a.timestamp - b.timestamp;
	}

	function matchesText(entry: LogEntry, currentTextFilter: string): boolean {
		return (
			!currentTextFilter ||
			entry.message.toLowerCase().includes(currentTextFilter.toLowerCase()) ||
			entry.service.toLowerCase().includes(currentTextFilter.toLowerCase())
		);
	}

	function refresh(currentFilter: string | string[] | null, currentTextFilter: string) {
		if (!terminal) return;
		terminal.clear();

		const currentLogs = getFilteredLogs(currentFilter);
		const multiServiceFilter = isMultiServiceFilter(currentFilter);
		const hasFilter = Array.isArray(currentFilter) ? currentFilter.length > 0 : !!currentFilter;

		terminal.write(formatLogBoundary('Recent history'));
		for (const entry of currentLogs.recent) {
			if (matchesText(entry, currentTextFilter)) {
				terminal.write(formatLog(entry, { hasFilter, isMultiServiceFilter: multiServiceFilter }));
			}
		}
		terminal.write(formatLogBoundary('Live'));
		for (const entry of currentLogs.live) {
			if (matchesText(entry, currentTextFilter)) {
				terminal.write(formatLog(entry, { hasFilter, isMultiServiceFilter: multiServiceFilter }));
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
			const unsubscribeLogs = liveLogs.subscribe((entry) => {
				if (terminal) {
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
						terminal.write(
							formatLog(entry, {
								hasFilter,
								isMultiServiceFilter: isMultiServiceFilter(filter)
							})
						);
					}
				}
			});
			const unsubscribeState = logStateChanged.subscribe(() => refresh(filter, textFilter));

			const resizeObserver = new ResizeObserver(() => {
				fitAddon.fit();
			});

			resizeObserver.observe(terminalContainer);

			cleanup = () => {
				unsubscribeLogs();
				unsubscribeState();
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
