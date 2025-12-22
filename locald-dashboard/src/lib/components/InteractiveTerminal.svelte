<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import '@xterm/xterm/css/xterm.css';
	import type { Terminal } from '@xterm/xterm';
	import type { FitAddon } from '@xterm/addon-fit';
	import { terminalTheme } from '$lib/theme';

	let { serviceName }: { serviceName: string } = $props();

	let terminalContainer: HTMLDivElement;
	let terminal: Terminal;
	let fitAddon: FitAddon;
	let socket: WebSocket;
	let resizeObserver: ResizeObserver;

	onMount(async () => {
		if (typeof window !== 'undefined') {
			const { Terminal } = await import('@xterm/xterm');
			const { FitAddon } = await import('@xterm/addon-fit');

			terminal = new Terminal({
				cursorBlink: true,
				fontSize: 14,
				fontFamily: 'Menlo, Monaco, "Courier New", monospace',
				theme: terminalTheme
			});

			fitAddon = new FitAddon();
			terminal.loadAddon(fitAddon);
			terminal.open(terminalContainer);
			fitAddon.fit();

			// Connect to WebSocket
			const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
			const wsUrl = `${protocol}//${window.location.host}/api/services/${serviceName}/pty`;
			socket = new WebSocket(wsUrl);
			socket.binaryType = 'arraybuffer';

			socket.onopen = () => {
				// Send initial resize
				const dims = {
					type: 'resize',
					rows: terminal.rows,
					cols: terminal.cols
				};
				socket.send(JSON.stringify(dims));

				terminal.onData((data) => {
					if (socket.readyState === WebSocket.OPEN) {
						const msg = {
							type: 'input',
							data: data
						};
						socket.send(JSON.stringify(msg));
					}
				});

				terminal.onResize((size) => {
					if (socket.readyState === WebSocket.OPEN) {
						const msg = {
							type: 'resize',
							rows: size.rows,
							cols: size.cols
						};
						socket.send(JSON.stringify(msg));
					}
				});
			};

			socket.onmessage = (event) => {
				if (event.data instanceof ArrayBuffer) {
					terminal.write(new Uint8Array(event.data));
				}
			};

			socket.onclose = () => {
				terminal.write('\r\n\x1b[31mConnection closed.\x1b[0m\r\n');
			};

			socket.onerror = (error) => {
				console.error('WebSocket error:', error);
				terminal.write('\r\n\x1b[31mConnection error.\x1b[0m\r\n');
			};

			resizeObserver = new ResizeObserver(() => {
				fitAddon.fit();
			});
			resizeObserver.observe(terminalContainer);
		}
	});

	onDestroy(() => {
		if (socket) {
			socket.close();
		}
		if (terminal) {
			terminal.dispose();
		}
		if (resizeObserver) {
			resizeObserver.disconnect();
		}
	});
</script>

<div class="terminal-container" bind:this={terminalContainer}></div>

<style>
	.terminal-container {
		width: 100%;
		height: 100%;
		background-color: #18181b; /* zinc-900 */
		padding: 8px;
		box-sizing: border-box;
		overflow: hidden;
	}
</style>
