import type { ProjectListEntry } from './api';
import type { ServiceStatus } from './types';

export type ProjectSection = ProjectListEntry['section'];
export type ServiceProject = { name: string; path: string | null; services: ServiceStatus[] };
export type ProjectGroup = ServiceProject & {
	kind: 'project';
	key: string;
	entry: ProjectListEntry | null;
	section: ProjectSection | null;
	checkoutLabel: string | null;
};
export type RackEntry = { kind: 'section'; section: ProjectSection; count: number } | ProjectGroup;

function projectName(entry: ProjectListEntry): string {
	return entry.project_name || entry.project_path.split('/').filter(Boolean).pop() || '/';
}

function serviceProjectKey(project: ServiceProject): string {
	return project.path ?? project.services[0]?.instance_id ?? project.name;
}

/** Shortest path suffix that identifies this checkout among same-named projects. */
export function checkoutLabel(path: string, peerPaths: string[]): string {
	const parts = path.split('/').filter(Boolean);
	const peers = peerPaths.filter((peer) => peer !== path);
	for (let length = 1; length <= parts.length; length += 1) {
		const suffix = parts.slice(-length).join('/');
		if (
			peers.every((peer) => peer.split('/').filter(Boolean).slice(-length).join('/') !== suffix)
		) {
			return suffix;
		}
	}
	return path;
}

export function buildRackEntries(
	active: ProjectListEntry[],
	alwaysOn: ProjectListEntry[],
	recent: ProjectListEntry[],
	serviceProjects: ServiceProject[],
	collapsed: ProjectSection[]
): RackEntry[] {
	const entries: RackEntry[] = [];
	const claimed = new Set<ServiceProject>();
	const attachments = [...active, ...alwaysOn, ...recent];
	const peers = [
		...attachments.map((entry) => ({ name: projectName(entry), path: entry.project_path })),
		...serviceProjects
	];
	const labelFor = (name: string, path: string | null) => {
		if (!path) return null;
		const paths = [
			...new Set(peers.filter((peer) => peer.name === name && peer.path).map((peer) => peer.path!))
		];
		return paths.length > 1 ? checkoutLabel(path, paths) : null;
	};

	function addSection(section: ProjectSection, list: ProjectListEntry[]) {
		if (list.length === 0) return;
		entries.push({ kind: 'section', section, count: list.length });
		for (const entry of list) {
			const name = projectName(entry);
			const exact = serviceProjects.find((project) => project.path === entry.project_path);
			// A legacy response without paths can only attach when its name is unambiguous.
			const legacy = serviceProjects.filter(
				(project) => project.path === null && project.name === name
			);
			const group =
				exact ??
				(legacy.length === 1 &&
				attachments.filter((other) => projectName(other) === name).length === 1
					? legacy[0]
					: undefined);
			if (group) claimed.add(group);
			if (collapsed.includes(section)) continue;
			entries.push({
				kind: 'project',
				key: entry.project_path,
				name,
				path: entry.project_path,
				entry,
				services: group?.services ?? [],
				section,
				checkoutLabel: labelFor(name, entry.project_path)
			});
		}
	}
	addSection('Active', active);
	addSection('AlwaysOn', alwaysOn);
	addSection('Recent', recent);
	for (const project of serviceProjects) {
		if (claimed.has(project)) continue;
		entries.push({
			...project,
			kind: 'project',
			key: serviceProjectKey(project),
			entry: null,
			section: null,
			checkoutLabel: labelFor(project.name, project.path)
		});
	}
	return entries;
}
