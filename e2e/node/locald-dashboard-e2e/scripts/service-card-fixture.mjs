import { createServer } from "node:http";
import { readFile, stat } from "node:fs/promises";
import { dirname, extname, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const buildRoot = resolve(here, "../../../../locald-dashboard/build");
const paths = ["/fixture/feature-a/shop", "/fixture/feature-b/shop"];
export const fixtureIds = [
  "00000000-0000-0000-0000-000000000001",
  "00000000-0000-0000-0000-000000000002",
];

function managed(index, serviceName, overrides = {}) {
  return {
    name: `shop:${serviceName}`,
    instance_id: fixtureIds[index],
    service_name: serviceName,
    service_type: "exec",
    pid: 1200 + index,
    port: 3000,
    status: "running",
    url: `https://${serviceName.replaceAll(":", "-")}.feature-${index ? "b" : "a"}.locald-fixture.invalid`,
    connection_url: null,
    domain: null,
    health_status: "healthy",
    health_source: "tcp",
    path: paths[index],
    workspace: "shop",
    constellation: null,
    warnings: [],
    ...overrides,
  };
}

export const publicationDefaults = {
  "waiting_for_publisher": {
    "explanation": "The stable service identity is declared, but no external publisher currently fulfills it.",
    "next_step": "Start the service with its owning workflow."
  },
  "checking_endpoint": {
    "explanation": "The owning workflow has published an exact endpoint, but locald has not authorized it for routing yet.",
    "next_step": "Wait for locald to verify the published endpoint."
  },
  "endpoint_unhealthy": {
    "explanation": "The owning workflow is publishing this service, but its exact endpoint is unhealthy.",
    "next_step": "Inspect the owning workflow and its `/api/health` endpoint."
  },
  "ready": {
    "explanation": "The owning workflow is publishing a healthy endpoint through this stable origin.",
    "next_step": null
  },
  "route_paused": {
    "explanation": "The project route is paused; locald is preserving this published origin without routing it.",
    "next_step": "Resume the project to allow its owning workflow to restore publication."
  },
  "instance_missing": {
    "explanation": "The worktree for this published service is missing; locald is preserving its stable origin without routing it.",
    "next_step": "Restore the worktree, or explicitly forget the project if this identity is no longer needed."
  }
};

function published(serviceName, state) {
  const { explanation, next_step: nextStep } = publicationDefaults[state];
  const origin = `https://${serviceName}.published.locald-fixture.invalid`;
  return managed(1, serviceName, {
    service_type: "published",
    status: "externally_managed",
    pid: null,
    port: null,
    // Deliberately different: the canonical publication origin owns navigation.
    url: `http://${serviceName}.endpoint.locald-fixture.invalid:45000`,
    publication: { state, origin, explanation, next_step: nextStep },
  });
}

export function fixtureServices() {
  return [
    managed(0, "web:preview", {
      url: "https://a-deliberately-long-web-service-hostname.feature-a.locald-fixture.invalid",
    }),
    managed(0, "api", { status: "stopped", pid: null, health_status: "unknown" }),
    managed(0, "docs", { status: "building", pid: null, health_status: "unknown" }),
    managed(0, "worker", { service_type: "worker", url: null, port: null }),
    managed(0, "database", { service_type: "postgres", url: null, connection_url: "postgres://fixture.invalid/shop" }),
    managed(1, "web:preview", { url: "http://web.feature-b.locald-fixture.invalid:54406" }),
    published("workbench", "ready"),
    published("storybook", "waiting_for_publisher"),
    published("design", "route_paused"),
  ];
}

function fixtureProjects() {
  return paths.map((project_path) => ({
    project_path,
    project_name: "shop",
    attachments: [],
    is_running: true,
    section: "Active",
    availability: {
      desired: true,
      state: "ready",
      always_on: false,
      paused: false,
      reasons: [],
      demands: [],
    },
  }));
}

const mime = {
  ".html": "text/html; charset=utf-8", ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8", ".json": "application/json", ".svg": "image/svg+xml",
  ".png": "image/png", ".ico": "image/x-icon", ".woff2": "font/woff2",
};

export async function startFixtureServer({ port = 0, root = buildRoot } = {}) {
  await stat(resolve(root, "index.html"));
  let services = fixtureServices();
  let requests = [];
  let unexpected = [];
  const streams = new Set();
  const send = (response, status, body) => {
    response.writeHead(status, { "Content-Type": "application/json", "Cache-Control": "no-store" });
    response.end(JSON.stringify(body));
  };
  const event = (response, message) => response.write(`data: ${JSON.stringify(message)}\n\n`);
  const broadcast = (message) => { for (const response of streams) event(response, message); };
  const server = createServer(async (request, response) => {
    const url = new URL(request.url, "http://fixture.invalid");
    try {
      if (request.method === "GET" && url.pathname === "/__fixture/health") return send(response, 200, { fixture: "service-cards" });
      if (request.method === "GET" && url.pathname === "/__fixture/requests") return send(response, 200, { requests, unexpected });
      if (request.method === "POST" && url.pathname === "/__fixture/reset") {
        services = fixtureServices(); requests = []; unexpected = [];
        return send(response, 200, { ok: true });
      }
      if (request.method === "POST" && url.pathname === "/__fixture/publication") {
        const state = url.searchParams.get("state");
        if (!Object.hasOwn(publicationDefaults, state)) return send(response, 400, { error: "Unknown fixture state" });
        const service = services.find((item) => item.service_name === "storybook");
        service.publication = { ...service.publication, state, ...publicationDefaults[state] };
        broadcast({ type: "ServiceUpdate", data: service });
        return send(response, 200, { ok: true });
      }
      if (request.method === "GET" && url.pathname === "/api/state") return send(response, 200, services);
      if (request.method === "GET" && url.pathname === "/api/projects") return send(response, 200, fixtureProjects());
      if (request.method === "GET" && url.pathname === "/api/events") {
        response.writeHead(200, { "Content-Type": "text/event-stream", "Cache-Control": "no-cache", Connection: "keep-alive" });
        response.flushHeaders();
        streams.add(response);
        event(response, { type: "LogReplayStarted" });
        for (const service of services) {
          event(response, { type: "Log", data: { timestamp: 1788652800, service: service.name,
            instance_id: service.instance_id, service_name: service.service_name, stream: "stdout",
            message: `Isolated fixture: ${service.name} in ${service.path}\r\n` } });
        }
        event(response, { type: "LogReplayFinished" });
        const heartbeat = setInterval(() => response.write(": fixture heartbeat\n\n"), 15000);
        response.on("close", () => { clearInterval(heartbeat); streams.delete(response); });
        return;
      }
      const action = url.pathname.match(/^\/api\/instances\/([^/]+)\/services\/([^/]+)\/(start|stop|restart|reset)$/);
      if (request.method === "POST" && action) {
        const [, encodedInstance, encodedName, kind] = action;
        const instance = decodeURIComponent(encodedInstance), name = decodeURIComponent(encodedName);
        const service = services.find((item) => item.instance_id === instance && item.service_name === name);
        if (service && service.service_type !== "published") {
          requests.push({ method: request.method, path: url.pathname, instance_id: instance, service_name: name, action: kind });
          if (kind === "start" || kind === "stop") service.status = kind === "stop" ? "stopped" : "running";
          send(response, 200, { ok: true });
          broadcast({ type: "ServiceUpdate", data: service });
          return;
        }
      }
      if (url.pathname.startsWith("/api/") || request.method !== "GET") {
        unexpected.push({ method: request.method, path: url.pathname });
        return send(response, 501, { error: "Unimplemented isolated fixture request; no request is forwarded." });
      }
      const pathname = decodeURIComponent(url.pathname);
      const file = resolve(root, `.${pathname === "/" ? "/index.html" : pathname}`);
      if (!file.startsWith(`${resolve(root)}${sep}`)) return send(response, 403, { error: "Outside fixture build" });
      const bytes = await readFile(file);
      response.writeHead(200, { "Content-Type": mime[extname(file)] ?? "application/octet-stream", "Cache-Control": "no-store" });
      response.end(bytes);
    } catch (error) {
      send(response, error.code === "ENOENT" ? 404 : 500, { error: String(error) });
    }
  });
  await new Promise((accept, reject) => { server.once("error", reject); server.listen(port, "127.0.0.1", accept); });
  return { server, url: `http://127.0.0.1:${server.address().port}`, close: async () => {
    for (const response of streams) response.end();
    await new Promise((accept, reject) => server.close((error) => error ? reject(error) : accept()));
  } };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const fixture = await startFixtureServer({ port: Number(process.env.SERVICE_CARD_FIXTURE_PORT ?? 47831) });
  console.log(`Isolated dashboard fixture: ${fixture.url}`);
  console.log("All lifecycle actions are simulated. Destinations use reserved .invalid names and cannot reach live projects.");
  for (const signal of ["SIGINT", "SIGTERM"]) process.once(signal, async () => { await fixture.close(); process.exit(0); });
}
