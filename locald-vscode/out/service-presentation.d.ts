import type { EnsuredServiceStatus, PublicationStatus, ServiceStatus } from "./plumbing.js";
export declare function isPublishedService(service: ServiceStatus): boolean;
export declare function managedServiceHealthSummary(services: ServiceStatus[]): {
    total: number;
    healthy: number;
    published: number;
};
export declare function servicesWithStableOrigins(services: ServiceStatus[]): ServiceStatus[];
export declare function serviceDisplayOrigin(service: ServiceStatus): string | undefined;
type PublicationAwareService = Pick<EnsuredServiceStatus, "service_type" | "publication">;
export declare function managedLifecycleServices<T extends PublicationAwareService>(services: T[]): T[];
export declare function publicationStateLabel(state: PublicationStatus["state"]): string;
export declare function serviceTooltipLines(service: ServiceStatus): string[];
export declare function openedServiceMessage(service: {
    url?: string | null;
    publication?: PublicationStatus;
}): string;
export declare function defaultServiceWithOrigin(services: EnsuredServiceStatus[]): EnsuredServiceStatus | undefined;
export declare function restartedServicesMessage(services: PublicationAwareService[], urls: string[]): string;
export {};
