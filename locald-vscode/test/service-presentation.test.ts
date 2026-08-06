import assert from "node:assert/strict";
import test from "node:test";
import type { EnsuredServiceStatus, ServiceStatus } from "../src/plumbing.js";
import {
  defaultServiceWithOrigin,
  managedLifecycleServices,
  managedServiceHealthSummary,
  openedServiceMessage,
  restartedServicesMessage,
  serviceDisplayOrigin,
  serviceTooltipLines,
  servicesWithStableOrigins,
} from "../src/service-presentation.js";

function service(overrides: Partial<ServiceStatus> = {}): ServiceStatus {
  return {
    name: "example:web",
    url: "https://example.localhost",
    port: 3000,
    status: "running",
    health_status: "Healthy",
    domain: "example.localhost",
    service_type: "exec",
    ...overrides,
  };
}

const published = service({
  name: "example:workbench",
  service_type: "published",
  status: "externally_managed",
  health_status: "Unknown",
  port: null,
  domain: "workbench.example.localhost",
  url: "https://workbench.example.localhost",
  publication: {
    state: "waiting_for_publisher",
    origin: "https://workbench.example.localhost",
    explanation: "Waiting for an external owner.",
    next_step: "Start it through the owning workflow.",
  },
});

test("published identities do not lower managed health", () => {
  assert.deepEqual(managedServiceHealthSummary([service(), published]), {
    total: 1,
    healthy: 1,
    published: 1,
  });
});

test("published stable origins remain openable while no publisher is active", () => {
  assert.deepEqual(servicesWithStableOrigins([published]), [published]);
});

test("published display origins preserve explicit sandbox HTTPS ports", () => {
  const sandbox = service({
    ...published,
    domain: "workbench.example.localhost",
    url: "https://workbench.example.localhost:8443",
    publication: {
      ...published.publication!,
      origin: "https://workbench.example.localhost:8443",
    },
  });
  assert.equal(
    serviceDisplayOrigin(sandbox),
    "https://workbench.example.localhost:8443",
  );
});

test("managed service tooltips preserve explicit sandbox HTTPS ports", () => {
  const sandbox = service({
    domain: "example.localhost",
    url: "https://example.localhost:8443",
  });
  assert.deepEqual(serviceTooltipLines(sandbox), [
    "● example:web  https://example.localhost:8443",
  ]);
});

test("published tooltips explain state and next step", () => {
  assert.deepEqual(serviceTooltipLines(published), [
    "◇ example:workbench  https://workbench.example.localhost — Waiting for publisher",
    "  Waiting for an external owner.",
    "  Next: Start it through the owning workflow.",
  ]);
});

test("opening a published origin reports its current availability", () => {
  assert.equal(
    openedServiceMessage(published),
    "Opened the stable origin https://workbench.example.localhost. Waiting for an external owner. Next: Start it through the owning workflow.",
  );
});

test("default open prefers a managed ready service over waiting publication guidance", () => {
  const managed: EnsuredServiceStatus = {
    name: "example:web",
    service_type: "exec",
    status: "running",
    health_status: "Healthy",
    url: "https://example.localhost",
  };
  assert.equal(
    defaultServiceWithOrigin([
      {
        name: published.name,
        service_type: published.service_type,
        status: published.status,
        health_status: published.health_status,
        url: published.url ?? undefined,
        publication: published.publication,
      },
      managed,
    ]),
    managed,
  );
});

test("published-only projects expose no managed restart targets", () => {
  const publishedEnsured: EnsuredServiceStatus = {
    name: published.name,
    service_type: published.service_type,
    status: published.status,
    health_status: published.health_status,
    url: published.url ?? undefined,
    publication: published.publication,
  };
  assert.deepEqual(managedLifecycleServices([publishedEnsured]), []);
  assert.equal(
    restartedServicesMessage(
      [publishedEnsured],
      ["https://workbench.example.localhost"],
    ),
    "No locald-managed services were restarted. 1 published service remains owned by the external workflow. https://workbench.example.localhost",
  );
});
