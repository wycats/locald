import { dirname, isAbsolute, relative, sep } from "node:path";

export class AmbiguousProjectError extends Error {
  constructor(projectPaths: string[]) {
    super(
      `multiple locald projects match this VS Code window; focus a file inside one project (${projectPaths.join(", ")})`,
    );
    this.name = "AmbiguousProjectError";
  }
}

export function selectProjectPath(
  configPaths: string[],
  activeFilePath?: string,
): string | undefined {
  const projectPaths = [...new Set(configPaths.map((path) => dirname(path)))];
  if (projectPaths.length === 0) {
    return undefined;
  }

  if (activeFilePath) {
    const matching = projectPaths
      .filter((path) => isPathWithin(path, activeFilePath))
      .sort((left, right) => right.length - left.length);
    if (matching.length > 0) {
      return matching[0];
    }
  }

  if (projectPaths.length === 1) {
    return projectPaths[0];
  }

  throw new AmbiguousProjectError(projectPaths.sort());
}

function isPathWithin(root: string, candidate: string): boolean {
  const path = relative(root, candidate);
  return (
    path === "" ||
    (path !== ".." &&
      !path.startsWith(`..${sep}`) &&
      !isAbsolute(path))
  );
}
