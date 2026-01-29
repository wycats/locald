import { toasts } from '$lib/stores/toasts';

export async function copyToClipboard(text: string, label: string): Promise<boolean> {
	try {
		await navigator.clipboard.writeText(text);
		toasts.success(`Copied ${label} to clipboard`);
		return true;
	} catch {
		toasts.error(`Failed to copy ${label}`);
		return false;
	}
}
