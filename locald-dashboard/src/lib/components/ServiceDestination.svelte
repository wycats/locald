<script lang="ts">
	/* eslint-disable svelte/no-navigation-without-resolve */
	import { onMount, tick } from 'svelte';
	import { MoreHorizontal, Copy, ExternalLink, X } from 'lucide-svelte';
	import { copyToClipboard } from '$lib/utils/clipboard';

	import type { ServiceStatus } from '$lib/types';
	import { pendingActions } from '$lib/stores/actions';
	import {
		isPublishedService,
		publicationGuidance,
		serviceActionName
	} from '$lib/service-presentation';
	import {
		startServiceWithFeedback,
		stopServiceWithFeedback,
		restartServiceWithFeedback,
		resetServiceWithFeedback
	} from '$lib/actions/service';

	export let identity: string;
	export let serviceName: string;
	export let destination: string | null;
	export let service: ServiceStatus;
	$: published = isPublishedService(service);
	$: pending = $pendingActions.some(
		(action) =>
			action.serviceName === serviceActionName(service) && action.instanceId === service.instance_id
	);

	let closeButton: HTMLButtonElement;
	let trigger: HTMLButtonElement;
	let panel: HTMLDivElement;
	let copyButton: HTMLButtonElement;
	let open = false;
	let copying = false;
	let copyResult = '';
	let copyAttempt = 0;
	let observedDestination: string | null = null;
	let left = 12;
	let top = 12;

	$: resetChangedDestination(destination);

	function resetChangedDestination(value: string | null) {
		if (value === observedDestination) return;
		observedDestination = value;
		copyAttempt += 1;
		copying = false;
		copyResult = '';
	}

	function positionPopover() {
		if (!panel?.matches(':popover-open') || !trigger) return;
		const anchor = trigger.getBoundingClientRect();
		const viewport = window.visualViewport;
		const viewportLeft = viewport?.offsetLeft ?? 0;
		const viewportTop = viewport?.offsetTop ?? 0;
		const width = viewport?.width ?? window.innerWidth;
		const height = viewport?.height ?? window.innerHeight;
		const inset = 12;
		panel.style.maxWidth = `${Math.max(0, width - inset * 2)}px`;
		panel.style.maxHeight = `${Math.max(0, height - inset * 2)}px`;
		const bounds = panel.getBoundingClientRect();
		left = Math.max(
			viewportLeft + inset,
			Math.min(anchor.left, viewportLeft + width - bounds.width - inset)
		);
		const below = anchor.bottom + 6;
		const preferred =
			below + bounds.height <= viewportTop + height - inset
				? below
				: anchor.top - bounds.height - 6;
		top = Math.max(
			viewportTop + inset,
			Math.min(preferred, viewportTop + height - bounds.height - inset)
		);
	}

	async function handleToggle() {
		open = panel.matches(':popover-open');
		copyAttempt += 1;
		copying = false;
		copyResult = '';
		if (open) {
			await tick();
			if (!panel.matches(':popover-open')) return;
			positionPopover();
			(destination ? copyButton : closeButton).focus({ preventScroll: true });
		} else if (panel.contains(document.activeElement)) {
			trigger.focus({ preventScroll: true });
		}
	}

	function closePopover() {
		panel.hidePopover();
		trigger.focus({ preventScroll: true });
	}

	function handleKeydown(event: KeyboardEvent) {
		// Address text and dialog controls retain their own keyboard behavior.
		event.stopPropagation();
		if (event.key === 'Escape') {
			event.preventDefault();
			closePopover();
		}
	}

	async function copyUrl() {
		if (copying || !destination) return;
		copying = true;
		const attempt = ++copyAttempt;
		const copiedDestination = destination;
		const copied = await copyToClipboard(copiedDestination, 'website URL');
		if (attempt !== copyAttempt) return;
		if (destination === copiedDestination && panel.matches(':popover-open')) {
			copyResult = copied
				? 'Copied URL'
				: 'Could not copy URL. Select the address and copy it manually.';
		}
		copying = false;
		await tick();
		positionPopover();
	}

	onMount(() => {
		window.addEventListener('resize', positionPopover);
		document.addEventListener('scroll', positionPopover, true);
		window.visualViewport?.addEventListener('resize', positionPopover);
		window.visualViewport?.addEventListener('scroll', positionPopover);
		return () => {
			window.removeEventListener('resize', positionPopover);
			document.removeEventListener('scroll', positionPopover, true);
			window.visualViewport?.removeEventListener('resize', positionPopover);
			window.visualViewport?.removeEventListener('scroll', positionPopover);
		};
	});
</script>

<div class="service-destination">
	{#if destination}
		<a
			class="service-url"
			href={destination}
			target="_blank"
			rel="noopener noreferrer"
			aria-label="Open {destination}"
			title="Open {destination}"
		>
			<span>Open</span><ExternalLink size={11} aria-hidden="true" />
		</a>
	{/if}
	<button
		bind:this={trigger}
		type="button"
		class="destination-details-trigger"
		popovertarget="destination-{identity}"
		aria-label="Service options for {serviceName}"
		aria-haspopup="dialog"
		aria-expanded={open}
		title="Service options"><MoreHorizontal size={12} aria-hidden="true" /></button
	>
</div>

<div
	bind:this={panel}
	id="destination-{identity}"
	class="destination-popover"
	popover="auto"
	role="dialog"
	aria-label="Service options for {serviceName}"
	tabindex="-1"
	style:left="{left}px"
	style:top="{top}px"
	on:toggle={handleToggle}
	on:keydown={handleKeydown}
>
	<div class="destination-heading">
		<span>{serviceName}</span>
		<button
			type="button"
			class="close-destination"
			bind:this={closeButton}
			aria-label="Close service options"
			on:click={closePopover}><X size={14} aria-hidden="true" /></button
		>
	</div>
	{#if service.publication}
		<div class="publication-guidance">
			<p>{publicationGuidance(service.publication).explanation}</p>
			{#if publicationGuidance(service.publication).next_step}<p>
					{publicationGuidance(service.publication).next_step}
				</p>{/if}
		</div>
	{/if}
	{#if destination}
		<p class="destination-full-url" data-testid="destination-url">{destination}</p>
		<button
			bind:this={copyButton}
			type="button"
			class="copy-url"
			disabled={copying}
			on:click={copyUrl}
			><Copy size={12} aria-hidden="true" />{copying ? 'Copying…' : 'Copy URL'}</button
		>
		<p class="copy-result" role="status">{copyResult}</p>
	{/if}
	{#if !published}
		<div class="service-actions">
			{#if service.status === 'running'}
				<button
					disabled={pending}
					aria-label="Restart {serviceName}"
					on:click={() =>
						restartServiceWithFeedback(serviceActionName(service), service.instance_id)}
					>Restart</button
				>
				<button
					disabled={pending}
					aria-label="Stop {serviceName}"
					on:click={() => stopServiceWithFeedback(serviceActionName(service), service.instance_id)}
					>Stop</button
				>
				<button
					disabled={pending}
					aria-label="Reset {serviceName}"
					on:click={() => resetServiceWithFeedback(serviceActionName(service), service.instance_id)}
					>Reset</button
				>
			{:else}
				<button
					disabled={pending}
					aria-label="Start {serviceName}"
					on:click={() => startServiceWithFeedback(serviceActionName(service), service.instance_id)}
					>Start</button
				>
			{/if}
		</div>
		<div class="process-info">PID: {service.pid || '—'} · Port: {service.port || '—'}</div>
	{/if}
</div>

<style>
	.service-actions {
		display: flex;
		gap: 8px;
		flex-wrap: wrap;
		margin-top: 12px;
		padding-top: 12px;
		border-top: 1px solid #3f3f46;
	}
	.service-actions button {
		border: 1px solid #52525b;
		border-radius: 4px;
		padding: 6px 10px;
		background: #27272a;
		color: #e4e4e7;
	}
	.service-actions button:disabled {
		opacity: 0.5;
		cursor: wait;
	}
	.process-info {
		color: #a1a1aa;
		font-size: 11px;
		margin-top: 10px;
	}
	.publication-guidance {
		color: #d4d4d8;
	}
	.publication-guidance p {
		margin: 8px 0;
	}

	.service-destination {
		display: flex;
		align-items: center;
		gap: 8px;
		min-width: 0;
	}
	.service-url {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		padding: 2px 0;
		color: #d4d4d8;
		font-size: 12px;
		text-decoration: none;
		border-radius: 2px;
	}
	.service-url:hover {
		color: #f8fafc;
		text-decoration: underline;
	}
	button {
		font: inherit;
		cursor: pointer;
	}
	.destination-details-trigger,
	.close-destination {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		min-width: 24px;
		min-height: 24px;
		padding: 3px;
		background: none;
		border: 0;
		border-radius: 3px;
		color: #a1a1aa;
	}
	.destination-details-trigger:hover,
	.close-destination:hover {
		color: #f8fafc;
		background: #27272a;
	}
	a:focus-visible,
	button:focus-visible {
		outline: 2px solid #60a5fa;
		outline-offset: 2px;
	}
	.destination-popover {
		position: fixed;
		inset: auto;
		margin: 0;
		box-sizing: border-box;
		width: min(360px, calc(100vw - 24px));
		max-height: calc(100vh - 24px);
		overflow: auto;
		padding: 12px;
		border: 1px solid #3f3f46;
		border-radius: 7px;
		background: #18181b;
		color: #e4e4e7;
		box-shadow: 0 8px 28px #0008;
		font-size: 12px;
	}
	.destination-heading {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 12px;
		font-weight: 600;
	}
	.destination-full-url {
		margin: 8px 0 12px;
		font-family: var(--font-mono, monospace);
		overflow-wrap: anywhere;
		user-select: all;
	}
	.copy-url {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		padding: 5px 8px;
		border: 1px solid #52525b;
		border-radius: 4px;
		background: #27272a;
		color: #e4e4e7;
	}
	.copy-url:disabled {
		opacity: 0.6;
		cursor: wait;
	}
	.copy-result {
		margin: 6px 0 0;
		color: #a1a1aa;
		font-size: 11px;
	}
	.copy-result:empty {
		margin: 0;
	}
</style>
