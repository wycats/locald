"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.isPublishedService = isPublishedService;
exports.managedServiceHealthSummary = managedServiceHealthSummary;
exports.servicesWithStableOrigins = servicesWithStableOrigins;
exports.serviceDisplayOrigin = serviceDisplayOrigin;
exports.managedLifecycleServices = managedLifecycleServices;
exports.publicationStateLabel = publicationStateLabel;
exports.serviceTooltipLines = serviceTooltipLines;
exports.openedServiceMessage = openedServiceMessage;
exports.defaultServiceWithOrigin = defaultServiceWithOrigin;
exports.restartedServicesMessage = restartedServicesMessage;
function isPublishedService(service) {
    return service.service_type === "published";
}
function managedServiceHealthSummary(services) {
    const managed = services.filter((service) => !isPublishedService(service));
    return {
        total: managed.length,
        healthy: managed.filter((service) => service.health_status === "Healthy")
            .length,
        published: services.length - managed.length,
    };
}
function servicesWithStableOrigins(services) {
    return services.filter((service) => service.url &&
        (service.status === "running" || isPublishedService(service)));
}
function serviceDisplayOrigin(service) {
    return (service.publication?.origin ?? service.url ?? service.domain ?? undefined);
}
function managedLifecycleServices(services) {
    return services.filter((service) => service.service_type !== "published" && !service.publication);
}
function publicationStateLabel(state) {
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
function serviceTooltipLines(service) {
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
function openedServiceMessage(service) {
    const url = service.publication?.origin ?? service.url ?? "the service origin";
    if (!service.publication) {
        return `Opened ${url} in Simple Browser.`;
    }
    const nextStep = service.publication.next_step
        ? ` Next: ${service.publication.next_step}`
        : "";
    return `Opened the stable origin ${url}. ${service.publication.explanation}${nextStep}`;
}
function defaultServiceWithOrigin(services) {
    return (services.find((service) => service.url && !service.publication) ??
        services.find((service) => service.url && service.publication?.state === "ready") ??
        services.find((service) => service.url));
}
function restartedServicesMessage(services, urls) {
    const published = services.filter((service) => service.publication).length;
    const managed = managedLifecycleServices(services).length;
    const publicationNote = published > 0
        ? ` ${published} published service${published === 1 ? " remains" : "s remain"} owned by the external workflow.`
        : "";
    const origins = urls.length > 0 ? ` ${urls.join(" ")}` : "";
    const outcome = managed > 0
        ? "Managed services restarted and ready."
        : "No locald-managed services were restarted.";
    return `${outcome}${publicationNote}${origins}`;
}
//# sourceMappingURL=service-presentation.js.map