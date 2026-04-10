let initPromise: Promise<void> | null = null;

export async function loadGhostty() {
	const ghostty = await import('ghostty-web');

	if (!initPromise) {
		initPromise = ghostty.init();
	}

	await initPromise;
	return ghostty;
}
