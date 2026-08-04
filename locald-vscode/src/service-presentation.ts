import type {
  EnsuredServiceStatus,
  PublicationStatus,
  ServiceStatus,
} from "./plumbing.js";

export function isPublishedService(service: ServiceStatus): boolean {
  return service.service_type === "published";
}

export function managedServiceHealthSummary(services: ServiceStatus[]): {
  total: number;
  healthy: number;
  published: number;
} {
  const managed = services.filter((service) => !isPublishedService(service));
  return {
    total: managed.length,
    healthy: managed.filter((service) => service.health_status === "Healthy")
      .length,
    published: services.length - managed.length,
  };
}

export function servicesWithStableOrigins(
  services: ServiceStatus[],
): ServiceStatus[] {
  return services.filter(
    (service) =>
      service.url &&
      (service.status === "running" || isPublishedService(service)),
  );
}

export function serviceDisplayOrigin(
  service: ServiceStatus,
): string | undefined {
  return (
    service.publication?.origin ?? service.url ?? service.domain ?? undefined
  );
}

type PublicationAwareService = Pick<
  EnsuredServiceStatus,
  "service_type" | "publication"
>;

export function managedLifecycleServices<T extends PublicationAwareService>(
  services: T[],
): T[] {
  return services.filter(
    (service) => service.service_type !== "published" && !service.publication,
  );
}

export function publicationStateLabel(
  state: PublicationStatus["state"],
): string {
  switch (state) {
    case "waiting_for_publisher":
      return "Waiting for publisher";
    case "checking_endpoint":
      return "Checking endpoint";
    case "endpoint_unhealthy":
      return "Endpoint unhealthy";
    case "ready":
      return "Ready";
    case "route_paused":
      return "Route paused";
    case "instance_missing":
      return "Worktree missing";
  }
}

export function serviceTooltipLines(service: ServiceStatus): string[] {
  const origin = serviceDisplayOrigin(service);
  const url = origin ? `  ${origin}` : "";
  if (!service.publication) {
    const icon = service.status === "running" ? "●" : "○";
    return [`${icon} ${service.name}${url}`];
  }

  const lines = [
    `◇ ${service.name}${url} — ${publicationStateLabel(service.publication.state)}`,
    `  ${service.publication.explanation}`,
  ];
  if (service.publication.next_step) {
    lines.push(`  Next: ${service.publication.next_step}`);
  }
  return lines;
}

export function openedServiceMessage(service: {
  url?: string | null;
  publication?: PublicationStatus;
}): string {
  const url =
    service.publication?.origin ?? service.url ?? "the service origin";
  if (!service.publication) {
    return `Opened ${url} in Simple Browser.`;
  }

  const nextStep = service.publication.next_step
    ? ` Next: ${service.publication.next_step}`
    : "";
  return `Opened the stable origin ${url}. ${service.publication.explanation}${nextStep}`;
}

export function defaultServiceWithOrigin(
  services: EnsuredServiceStatus[],
): EnsuredServiceStatus | undefined {
  return (
    services.find((service) => service.url && !service.publication) ??
    services.find(
      (service) => service.url && service.publication?.state === "ready",
    ) ??
    services.find((service) => service.url)
  );
}

export function restartedServicesMessage(
  services: PublicationAwareService[],
  urls: string[],
): string {
  const published = services.filter((service) => service.publication).length;
  const managed = managedLifecycleServices(services).length;
  const publicationNote =
    published > 0
      ? ` ${published} published service${published === 1 ? " remains" : "s remain"} owned by the external workflow.`
      : "";
  const origins = urls.length > 0 ? ` ${urls.join(" ")}` : "";
  const outcome =
    managed > 0
      ? "Managed services restarted and ready."
      : "No locald-managed services were restarted.";
  return `${outcome}${publicationNote}${origins}`;
}
