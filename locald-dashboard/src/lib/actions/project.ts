import { pauseProject, resumeProject, setProjectAlwaysOn, type ProjectListEntry } from '$lib/api';
import { projectList } from '$lib/stores/projects';
import { services } from '$lib/stores/services';
import { toasts } from '$lib/stores/toasts';

async function refreshProjectSurfaces() {
	await Promise.all([projectList.refresh(), services.refresh()]);
}

function projectName(project: ProjectListEntry): string {
	return project.project_name || project.project_path.split('/').pop() || 'project';
}

export async function resumeProjectWithFeedback(project: ProjectListEntry): Promise<boolean> {
	try {
		await resumeProject(project.project_path);
		await refreshProjectSurfaces();
		toasts.success(`Resumed ${projectName(project)}`);
		return true;
	} catch (error) {
		toasts.error(error instanceof Error ? error.message : String(error));
		return false;
	}
}

export async function pauseProjectWithFeedback(project: ProjectListEntry): Promise<boolean> {
	try {
		await pauseProject(project.project_path);
		await refreshProjectSurfaces();
		toasts.success(`Paused ${projectName(project)}`);
		return true;
	} catch (error) {
		toasts.error(error instanceof Error ? error.message : String(error));
		return false;
	}
}

export async function setProjectAlwaysOnWithFeedback(
	project: ProjectListEntry,
	enabled: boolean
): Promise<boolean> {
	try {
		await setProjectAlwaysOn(project.project_path, enabled);
		await refreshProjectSurfaces();
		toasts.success(`${enabled ? 'Enabled' : 'Disabled'} Always On for ${projectName(project)}`);
		return true;
	} catch (error) {
		toasts.error(error instanceof Error ? error.message : String(error));
		return false;
	}
}
