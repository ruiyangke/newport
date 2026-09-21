export interface Container {
  id: string;
  name: string;
  image: string;
  state: string;
  status: string;
  ports: string;
  composeProject?: string;
  composeService?: string;
  composeOneoff?: string;
}
export interface ComposeProject {
  name: string;
  containers: Container[];
  services: string[];
  running: number;
}
export function groupContainers(containers: Container[]) {
  const projects = new Map<string, Container[]>();
  const standalone: Container[] = [];
  for (const c of containers) {
    if (!c.composeProject) {
      standalone.push(c);
      continue;
    }
    const group = projects.get(c.composeProject) ?? [];
    group.push(c);
    projects.set(c.composeProject, group);
  }
  return {
    projects: [...projects]
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([name, containers]): ComposeProject => ({
        name,
        containers: containers.sort(
          (a, b) =>
            (a.composeService ?? "").localeCompare(b.composeService ?? "") ||
            a.name.localeCompare(b.name),
        ),
        services: [
          ...new Set(
            containers
              .filter((c) => c.composeOneoff?.toLowerCase() !== "true")
              .map((c) => c.composeService)
              .filter((s): s is string => !!s),
          ),
        ],
        running: containers.filter((c) => c.state === "running").length,
      })),
    standalone: standalone.sort((a, b) => a.name.localeCompare(b.name)),
  };
}
