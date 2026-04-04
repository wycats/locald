import { writable, derived } from 'svelte/store';
import { getProjects, type ProjectListEntry } from '$lib/api';

function createProjectsStore() {
	const { subscribe, set } = writable<ProjectListEntry[]>([]);

	return {
		subscribe,
		set,
		refresh: async () => {
			try {
				const projects = await getProjects();
				set(projects);
			} catch {
				// If the daemon isn't running or the endpoint isn't available yet,
				// fall back to an empty list. The SSE reconnect will refresh later.
				set([]);
			}
		}
	};
}

export const projectList = createProjectsStore();

export const activeProjects = derived(projectList, ($list) =>
	$list.filter((p) => p.section === 'Active')
);

export const alwaysOnProjects = derived(projectList, ($list) =>
	$list.filter((p) => p.section === 'AlwaysOn')
);

export const recentProjects = derived(projectList, ($list) =>
	$list.filter((p) => p.section === 'Recent')
);
