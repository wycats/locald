export const LOCALD_CONFIG_GLOB = "**/locald.toml";
export const LOCALD_CONFIG_EXCLUDE =
  "**/{.git,node_modules,target,dist,build,.next}/**";

export interface ProjectConfigUri {
  fsPath: string;
}

export type ProjectConfigFinder = (
  include: string,
  exclude: string,
) => PromiseLike<readonly ProjectConfigUri[]>;

export async function findProjectConfigPaths(
  findFiles: ProjectConfigFinder,
): Promise<string[]> {
  const configs = await findFiles(
    LOCALD_CONFIG_GLOB,
    LOCALD_CONFIG_EXCLUDE,
  );
  return configs.map((config) => config.fsPath);
}
