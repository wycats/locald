import { test, expect, type Locator, type Page } from "@playwright/test";

const ids = [
  "00000000-0000-0000-0000-000000000001",
  "00000000-0000-0000-0000-000000000002",
];
const longDestination = "https://a-deliberately-long-web-service-hostname.feature-a.locald-fixture.invalid";
const row = (page: Page, index: number, name: string) => page.locator(
  `[data-testid="service-row"][data-service-key="${ids[index]}/${name}"]`,
);
const inspect = (item: Locator, name: string) => item.getByRole("button", { name: `Inspect shop:${name}`, exact: true });
const addressButton = (item: Locator, name: string) => item.getByRole("button", { name: `Service options for shop:${name}`, exact: true });
const addressDialog = (item: Locator, name: string) => item.getByRole("dialog", { name: `Service options for shop:${name}`, exact: true });

test.beforeEach(async ({ page, context, request, baseURL }) => {
  await request.post("/__fixture/reset");
  // Context routing also catches the first request of target=_blank popups.
  // Only this owned fixture origin reaches the network.
  await context.route("**/*", async (route) => {
    const url = new URL(route.request().url());
    if (url.origin === baseURL || !["http:", "https:"].includes(url.protocol)) {
      await route.continue();
    } else if (url.hostname.endsWith(".locald-fixture.invalid")) {
      await route.fulfill({ status: 200, contentType: "text/html", body: "<!doctype html><title>Isolated fixture destination</title><p>Mock service destination; no live service was contacted.</p>" });
    } else {
      await route.abort("blockedbyclient");
      throw new Error(`Unexpected external request: ${url.origin}${url.pathname}`);
    }
  });
  await page.goto("/");
  await expect(page.locator("body")).toHaveAttribute("data-sse-connected", "true");
  await expect(page.getByTestId("service-row")).toHaveCount(9);
});

test.afterEach(async ({ request }) => {
  const recorded = await (await request.get("/__fixture/requests")).json();
  expect(recorded.unexpected).toEqual([]);
});

async function geometry(item: Locator) {
  return item.evaluate((element) => {
    const bounds = (node: Element) => {
      const { x, y, width, height } = node.getBoundingClientRect();
      return { x, y, width, height };
    };
    return {
      row: bounds(element),
      inspect: bounds(element.querySelector(".service-inspect")!),
      destination: bounds(element.querySelector(".service-url")!),
      address: bounds(element.querySelector('button[aria-label^="Service options for "]')!),
    };
  });
}

function assertSeparate(a: { x: number; y: number; width: number; height: number }, b: typeof a) {
  const overlapsX = Math.min(a.x + a.width, b.x + b.width) - Math.max(a.x, b.x);
  const overlapsY = Math.min(a.y + a.height, b.y + b.height) - Math.max(a.y, b.y);
  expect(overlapsX <= 0.5 || overlapsY <= 0.5).toBe(true);
}

for (const viewport of [{ width: 945, height: 963 }, { width: 1440, height: 1000 }, { width: 640, height: 963 }, { width: 390, height: 844 }]) {
  test(`persistent destinations and controls have independent geometry at ${viewport.width}px`, async ({ page }, testInfo) => {
    await page.setViewportSize(viewport);
    const item = row(page, 0, "web:preview");
    await item.scrollIntoViewIfNeeded();
    const initial = await geometry(item);
    assertSeparate(initial.inspect, initial.destination);
    assertSeparate(initial.destination, initial.address);
    assertSeparate(initial.inspect, initial.address);
    for (const bounds of [initial.inspect, initial.destination, initial.address ]) {
      expect(bounds.width).toBeGreaterThan(0);
      expect(bounds.x).toBeGreaterThanOrEqual(initial.row.x);
      expect(bounds.x + bounds.width).toBeLessThanOrEqual(initial.row.x + initial.row.width + 0.5);
    }
    await item.hover();
    expect(await geometry(item)).toEqual(initial);
    await item.getByRole("link").focus();
    expect(await geometry(item)).toEqual(initial);
    await expect(item.getByRole("link")).toHaveAttribute("href", longDestination);
    await expect(item.getByRole("link")).toHaveAccessibleName(`Open ${longDestination}`);
    await expect(item.getByRole("link")).toHaveText("Open");
    expect(await item.innerText()).not.toContain("locald-fixture.invalid");
    const icon = await item.getByRole("link").locator("svg").boundingBox();
    expect(icon?.width).toBeGreaterThan(0);
    expect(icon!.x + icon!.width).toBeLessThanOrEqual(initial.destination.x + initial.destination.width + 0.5);
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth);
    expect(overflow).toBe(false);
    await page.screenshot({ path: testInfo.outputPath(`service-cards-${viewport.width}.png`), fullPage: true });
  });
}

for (const width of [1027, 390]) {
  test(`project hierarchy keeps services closer to their parent than the next project at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1203 });
    const headers = page.locator(".rack-group-header");
    const firstHeader = await headers.nth(0).boundingBox();
    const nextHeader = await headers.nth(1).boundingBox();
    const firstService = await row(page, 0, "web:preview").boundingBox();
    const lastService = await row(page, 0, "database").boundingBox();
    const title = await headers.nth(0).locator(".group-title").boundingBox();
    const serviceName = await row(page, 0, "web:preview").locator(".service-name").boundingBox();
    expect(serviceName!.x).toBeGreaterThan(title!.x);
    const withinProjectGap = firstService!.y - (firstHeader!.y + firstHeader!.height);
    const betweenProjectsGap = nextHeader!.y - (lastService!.y + lastService!.height);
    expect(withinProjectGap).toBeGreaterThanOrEqual(0);
    expect(betweenProjectsGap).toBeGreaterThan(withinProjectGap);
    await expect(headers.nth(0).locator(".group-title")).toHaveCSS("text-transform", "none");
    await expect(headers.nth(0).getByTestId("checkout-label")).toHaveText("feature-a/shop");
  });
}

test("same-name checkouts are distinguishable and all lifecycle states retain truthful destinations", async ({ page }) => {
  const labels = page.getByTestId("checkout-label");
  await expect(labels).toHaveCount(2);
  await expect(labels.nth(0)).toHaveText("feature-a/shop");
  await expect(labels.nth(1)).toHaveText("feature-b/shop");
  await expect(labels.nth(0)).toHaveAttribute("title", "/fixture/feature-a/shop");
  await expect(labels.nth(1)).toHaveAttribute("title", "/fixture/feature-b/shop");
  await expect(page.getByTestId("service-row").getByTestId("checkout-label")).toHaveCount(0);
  for (const name of ["api", "docs"]) {
    await expect(row(page, 0, name).getByRole("link")).toHaveAttribute("href", `https://${name}.feature-a.locald-fixture.invalid`);
  }
  for (const name of ["worker", "database"]) {
    await expect(row(page, 0, name).getByRole("link")).toHaveCount(0);
    await expect(addressButton(row(page, 0, name), name)).toHaveCount(1);
    await addressButton(row(page, 0, name), name).click();
    await expect(addressDialog(row(page, 0, name), name).getByTestId("destination-url")).toHaveCount(0);
    await expect(addressDialog(row(page, 0, name), name).getByRole("button", { name: `Stop shop:${name}`, exact: true })).toBeVisible();
    await page.keyboard.press("Escape");
  }
  await expect(row(page, 1, "web:preview").getByRole("link")).toHaveAttribute("href", "http://web.feature-b.locald-fixture.invalid:54406");
  for (const name of ["workbench", "storybook", "design"]) {
    const item = row(page, 1, name);
    await expect(item.getByRole("link")).toHaveAttribute("href", `https://${name}.published.locald-fixture.invalid`);
    await expect(item.getByRole("button", { name: /^(Inspect|Start|Stop|Restart|Reset|More) / })).toHaveCount(0);
    await expect(item.locator(".publication-guidance")).toBeHidden();
    await addressButton(item, name).click();
    await expect(item.locator(".publication-guidance")).toBeVisible();
    await expect(item.locator(".publication-guidance")).not.toBeEmpty();
    await page.keyboard.press("Escape");
  }
});

test("compact rows prioritize names and keep publication explanations behind disclosure", async ({ page }, testInfo) => {
  const web = row(page, 0, "web:preview");
  const worker = row(page, 0, "worker");
  await expect(worker).not.toContainText("No web address");
  await expect(worker.locator(".status-dot")).toHaveCount(0);
  await expect(web.getByRole("link")).toHaveText("Open");
  for (const item of [web, worker]) {
    expect(await item.innerText()).not.toContain("locald-fixture.invalid");
    expect(await item.innerText()).not.toContain("feature-a/shop");
  }
  const linkStyle = await web.getByRole("link").evaluate((element) => {
    const style = getComputedStyle(element);
    return { background: style.backgroundColor, border: style.borderTopWidth };
  });
  expect(linkStyle).toEqual({ background: "rgba(0, 0, 0, 0)", border: "0px" });
  const publication = row(page, 1, "storybook");
  await expect(publication.locator(".publication-guidance")).toBeHidden();
  await addressButton(publication, "storybook").press("Enter");
  await expect(addressButton(publication, "storybook")).toHaveAttribute("aria-expanded", "true");
  await expect(publication.locator(".publication-guidance")).toContainText("Start the service in that application.");
  await page.screenshot({ path: testInfo.outputPath("publication-expanded.png") });
  await page.keyboard.press("Escape");
  await expect(addressButton(publication, "storybook")).toHaveAttribute("aria-expanded", "false");
  await expect(publication.locator(".publication-guidance")).toBeHidden();
});

test("inspect Enter and Space each toggle only their exact colon-containing service", async ({ page, request }) => {
  const item = row(page, 0, "web:preview");
  const button = inspect(item, "web:preview");
  await button.focus();
  await button.press("Enter");
  await expect(button).toHaveAttribute("aria-pressed", "true");
  const selectionColor = await item.evaluate(el => getComputedStyle(el).backgroundColor);
  await item.hover();
  await expect(item).toHaveCSS("background-color", selectionColor);
  await expect.poll(() => new URL(page.url()).searchParams.get("monitor")).toBe(`${ids[0]}/web:preview`);
  await expect(inspect(row(page, 1, "web:preview"), "web:preview")).toHaveAttribute("aria-pressed", "false");
  await button.press("Space");
  await expect(button).toHaveAttribute("aria-pressed", "false");
  await expect.poll(() => new URL(page.url()).searchParams.get("monitor")).toBeNull();
  expect((await (await request.get("/__fixture/requests")).json()).requests).toEqual([]);
});

for (const [state, label, explanation, nextStep] of [
  ["ready", "Ready", "Another application runs this service. It is ready to open at this address.", null],
  ["waiting_for_publisher", "Waiting for app", "Another application starts this service. locald is waiting for it to connect.", "Start the service in that application."],
  ["checking_endpoint", "Checking connection", "The application has connected. locald is checking whether the service can receive requests.", "Wait for locald to finish checking the connection."],
  ["endpoint_unhealthy", "Unavailable", "The application has connected, but the service is failing its health check.", "Check the application and its /api/health endpoint."],
  ["route_paused", "Paused", "The project is paused. locald is keeping this address but is not sending traffic to the service.", "Resume the project to restore access."],
  ["instance_missing", "Worktree missing", "The worktree is missing. locald is keeping this address but is not sending traffic to the service.", "Restore the worktree, or explicitly forget the project if this identity is no longer needed."],
] as const) {
  test(`app-managed ${state} explains readiness without managed controls`, async ({ page, request }) => {
    expect((await request.post(`/__fixture/publication?state=${state}`)).ok()).toBe(true);
    const item = row(page, 1, "storybook");
    await expect(page.getByTestId("service-row")).toHaveCount(9);
    await expect(item.locator(".publication-state")).toHaveText(label);
    await addressButton(item, "storybook").click();
    await expect(item.locator(".publication-guidance p").first()).toHaveText(explanation);
    await expect(item.locator(".publication-guidance p")).toHaveCount(nextStep ? 2 : 1);
    if (nextStep) await expect(item.locator(".publication-guidance p").nth(1)).toHaveText(nextStep);
    await expect(item.getByRole("link")).toHaveAttribute("href", "https://storybook.published.locald-fixture.invalid");
    await expect(item.getByRole("button", { name: /^(Inspect|Start|Stop|Restart|Reset|More) / })).toHaveCount(0);
    expect((await (await request.get("/__fixture/requests")).json()).requests).toEqual([]);
  });
}

for (const width of [945, 390]) {
  test(`healthy project headers show identity without repeated operational metadata at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 963 });
    const headers = page.locator(".rack-group-header");
    await expect(headers).toHaveCount(2);
    await expect(headers.nth(1)).toContainText("feature-b/shop");
    await expect(headers.nth(1)).not.toContainText("running");
    await expect(headers.nth(1).getByRole("button")).toHaveCount(1);
    for (const item of [row(page, 0, "web:preview"), row(page, 0, "database")]) {
      const branch = await item.evaluate(el => ({
        stem: getComputedStyle(el, "::before").borderLeftWidth,
        twig: getComputedStyle(el, "::after").borderTopWidth
      }));
      expect(branch).toEqual({stem:"1px",twig:"1px"});
    }
  });
}

test("project failures stay visible and the heading opens exact availability details", async ({ page, request }, testInfo) => {
  const projects = await (await request.get("/api/projects")).json();
  projects[1].availability.state = "failed";
  projects[1].availability.last_error = "Fixture startup failed; locald will retry automatically.";
  await page.route("**/api/projects", route => route.fulfill({ json: projects }));
  await page.reload();
  const headers = page.locator(".rack-group-header");
  await expect(headers.nth(1).locator(".group-issue")).toHaveText("Failed");
  await expect(headers.nth(0).locator(".group-issue")).toHaveCount(0);
  await page.screenshot({ path: testInfo.outputPath("project-failure-tree.png") });
  await headers.nth(1).getByRole("button").click();
  await expect(headers.nth(1).getByRole("button")).toHaveAttribute("aria-pressed", "true");
  await expect(headers.nth(0).getByRole("button")).toHaveAttribute("aria-pressed", "false");
  await expect.poll(() => new URL(page.url()).searchParams.get("project")).toBe("/fixture/feature-b/shop");
  await expect(page.locator(".availability-copy")).toContainText(projects[1].availability.last_error);
  await expect(page.locator(".availability-actions").getByRole("button", { name: "Resume", exact: true })).toBeVisible();
  await page.setViewportSize({ width: 390, height: 844 });
  const copy = await page.locator(".availability-copy").boundingBox();
  const actions = await page.locator(".availability-actions").boundingBox();
  expect(copy!.width).toBeGreaterThan(200);
  expect(actions!.y).toBeGreaterThanOrEqual(copy!.y + copy!.height);
  await page.screenshot({ path: testInfo.outputPath("project-details-390.png") });
  expect((await (await request.get("/__fixture/requests")).json()).requests).toEqual([]);
});

test("Recent begins collapsed and expands without losing exact service identities", async ({ page, request }) => {
  const projects = await (await request.get("/api/projects")).json();
  projects[1].section = "Recent";
  await page.route("**/api/projects", route => route.fulfill({ json: projects }));
  await page.reload();
  const recent = page.getByRole("button", { name: /^Recent · 1 project$/i });
  await expect(recent).toHaveAttribute("aria-expanded", "false");
  await expect(row(page, 1, "web:preview")).toHaveCount(0);
  await recent.click();
  await expect(recent).toHaveAttribute("aria-expanded", "true");
  await expect(row(page, 1, "web:preview")).toBeVisible();
  await expect(row(page, 0, "web:preview")).toBeVisible();
});

test("Tab reaches the persistent link and native link activation leaves selection unchanged", async ({ page, request }) => {
  const item = row(page, 0, "web:preview");
  const button = inspect(item, "web:preview");
  await button.click();
  const selection = page.url();
  await button.focus();
  await page.keyboard.press("Tab");
  const link = item.getByRole("link");
  await expect(link).toBeFocused();
  expect(await link.evaluate((element) => element.matches(":focus-visible"))).toBe(true);
  const focusStyle = await link.evaluate((element) => ({ outline: getComputedStyle(element).outlineStyle, width: getComputedStyle(element).outlineWidth, shadow: getComputedStyle(element).boxShadow }));
  expect((focusStyle.outline !== "none" && focusStyle.width !== "0px") || focusStyle.shadow !== "none").toBe(true);
  await link.press("Space");
  expect(page.url()).toBe(selection);
  for (const activate of [() => link.press("Enter"), () => link.click()]) {
    const popupPromise = page.waitForEvent("popup");
    await activate();
    const popup = await popupPromise;
    await expect(popup).toHaveTitle("Isolated fixture destination");
    expect(popup.url()).toBe(`${longDestination}/`);
    await popup.close();
    expect(page.url()).toBe(selection);
    await expect(button).toHaveAttribute("aria-pressed", "true");
  }
  expect((await (await request.get("/__fixture/requests")).json()).requests).toEqual([]);
});

test("address popover is keyboard accessible and Escape and close restore trigger focus", async ({ page, request }) => {
  const item = row(page, 0, "web:preview");
  const trigger = addressButton(item, "web:preview");
  const dialog = addressDialog(item, "web:preview");
  const selection = page.url();
  await item.getByRole("link").focus();
  await page.keyboard.press("Tab");
  await expect(trigger).toBeFocused();
  for (const key of ["Enter", "Space"]) {
    await trigger.press(key);
    await expect(dialog).toBeVisible();
    await expect(dialog.getByTestId("destination-url")).toHaveText(longDestination);
    await expect(dialog.getByRole("button", { name: "Copy URL", exact: true })).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(dialog).toBeHidden();
    await expect(trigger).toBeFocused();
    expect(page.url()).toBe(selection);
  }
  await trigger.click();
  await dialog.getByRole("button", { name: "Close service options", exact: true }).click();
  await expect(dialog).toBeHidden();
  await expect(trigger).toBeFocused();
  expect(page.url()).toBe(selection);
  expect((await (await request.get("/__fixture/requests")).json()).requests).toEqual([]);
});

test("address popover light dismiss preserves independent pointer actions and non-navigation focus", async ({ page, request }) => {
  const item = row(page, 0, "web:preview");
  const dialog = addressDialog(item, "web:preview");
  const trigger = addressButton(item, "web:preview");
  const otherRow = row(page, 1, "web:preview");
  const selection = page.url();
  // More opens local UI without goto(), so this directly proves that light
  // dismissal preserves the newly focused outside control.
  const more = addressButton(otherRow, "web:preview");
  await trigger.click();
  await expect(dialog).toBeVisible();
  await more.click();
  await expect(dialog).toBeHidden();
  await expect(addressDialog(otherRow, "web:preview").getByRole("button", {name:"Copy URL",exact:true})).toBeFocused();
  await expect(more).toHaveAttribute("aria-expanded", "true");
  expect(page.url()).toBe(selection);

  await addressButton(item, "web:preview").click();
  await expect(dialog).toBeVisible();
  const otherInspect = inspect(otherRow, "web:preview");
  await otherInspect.click();
  await expect(dialog).toBeHidden();
  await expect(otherInspect).toHaveAttribute("aria-pressed", "true");
  await expect(inspect(item, "web:preview")).toHaveAttribute("aria-pressed", "false");
  await expect.poll(() => new URL(page.url()).searchParams.get("monitor")).toBe(`${ids[1]}/web:preview`);
  // +page.svelte updateUrl calls goto without keepFocus: SvelteKit deliberately
  // resets focus after this URL navigation. The popover must leave that route
  // policy intact and must not restore its own address trigger over it.
  await expect(trigger).not.toBeFocused();
  expect((await (await request.get("/__fixture/requests")).json()).requests).toEqual([]);
});

for (const fail of [false, true]) {
  test(`copying full address reports ${fail ? "failure with manual recovery" : "success"} without changing selection`, async ({ page, request }) => {
    // This stub belongs only to the isolated Playwright page. It never calls the
    // system clipboard, including in the success case.
    await page.evaluate((fail) => {
      const state = window as unknown as { fixtureCopiedUrls: string[] };
      state.fixtureCopiedUrls = [];
      Object.defineProperty(navigator, "clipboard", { configurable: true, value: {
        writeText: async (value: string) => {
          state.fixtureCopiedUrls.push(value);
          if (fail) throw new DOMException("Fixture clipboard denied", "NotAllowedError");
        },
      } });
    }, fail);
    const item = row(page, 0, "web:preview");
    const selection = page.url();
    await addressButton(item, "web:preview").click();
    const dialog = addressDialog(item, "web:preview");
    await expect(dialog.getByTestId("destination-url")).toHaveText(longDestination);
    await dialog.getByRole("button", { name: "Copy URL", exact: true }).click();
    await expect(dialog.getByRole("status")).toHaveText(fail
      ? "Could not copy URL. Select the address and copy it manually."
      : "Copied URL");
    expect(await page.evaluate(() => (window as unknown as { fixtureCopiedUrls: string[] }).fixtureCopiedUrls)).toEqual([longDestination]);
    await expect(dialog).toBeVisible();
    expect(page.url()).toBe(selection);
    expect((await (await request.get("/__fixture/requests")).json()).requests).toEqual([]);
  });
}

for (const width of [390, 640]) {
  test(`full address popover stays inside the ${width}px viewport`, async ({ page }, testInfo) => {
    await page.setViewportSize({ width, height: 844 });
    const item = row(page, 0, "web:preview");
    await addressButton(item, "web:preview").click();
    const dialog = addressDialog(item, "web:preview");
    await expect(dialog).toBeVisible();
    const bounds = await dialog.boundingBox();
    expect(bounds!.x).toBeGreaterThanOrEqual(0);
    expect(bounds!.y).toBeGreaterThanOrEqual(0);
    expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(width);
    expect(bounds!.y + bounds!.height).toBeLessThanOrEqual(844);
    const address = dialog.getByTestId("destination-url");
    await expect(address).toHaveText(longDestination);
    expect(await address.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
    const copyBounds = await dialog.getByRole("button", { name: "Copy URL", exact: true }).boundingBox();
    expect(copyBounds!.x).toBeGreaterThanOrEqual(bounds!.x);
    expect(copyBounds!.x + copyBounds!.width).toBeLessThanOrEqual(bounds!.x + bounds!.width);
    await page.screenshot({ path: testInfo.outputPath(`address-popover-${width}.png`), fullPage: true });
  });
}

for (const key of ["Enter", "Space"] as const) {
  test(`Restart ${key} submits one instance-scoped request and preserves selection`, async ({ page, request }) => {
    const item = row(page, 1, "web:preview");
    const selection = page.url();
    await addressButton(item, "web:preview").click();
    const action = item.getByRole("button", { name: "Restart shop:web:preview", exact: true });
    const response = page.waitForResponse((response) => response.request().method() === "POST");
    await action.focus();
    await action.press(key);
    expect((await response).ok()).toBe(true);
    await expect(action).toBeEnabled();
    const records = (await (await request.get("/__fixture/requests")).json()).requests;
    expect(records).toEqual([{ method: "POST", path: `/api/instances/${ids[1]}/services/web%3Apreview/restart`, instance_id: ids[1], service_name: "web:preview", action: "restart" }]);
    expect(page.url()).toBe(selection);
    await expect(inspect(item, "web:preview")).toHaveAttribute("aria-pressed", "false");
  });
}

test("Start and More/Stop pointer actions target only the chosen instance", async ({ page, request }) => {
  const stopped = row(page, 0, "api");
  await addressButton(stopped, "api").click();
  await stopped.getByRole("button", { name: "Start shop:api", exact: true }).click();
  await expect.poll(async () => (await (await request.get("/__fixture/requests")).json()).requests.length).toBe(1);
  // Finish this menu interaction before targeting a row that may sit behind
  // the native top-layer popover at denser sidebar spacing.
  await addressDialog(stopped, "api").getByRole("button", { name: "Close service options", exact: true }).click();
  await expect(addressDialog(stopped, "api")).toBeHidden();
  const running = row(page, 1, "web:preview");
  await addressButton(running, "web:preview").click();
  await running.getByRole("button", { name: "Stop shop:web:preview", exact: true }).click();
  await expect.poll(async () => (await (await request.get("/__fixture/requests")).json()).requests.length).toBe(2);
  const records = (await (await request.get("/__fixture/requests")).json()).requests;
  expect(records.map(({ instance_id, service_name, action }: { instance_id: string; service_name: string; action: string }) => ({ instance_id, service_name, action }))).toEqual([
    { instance_id: ids[0], service_name: "api", action: "start" },
    { instance_id: ids[1], service_name: "web:preview", action: "stop" },
  ]);
  expect(new URL(page.url()).searchParams.get("monitor")).toBeNull();
});
