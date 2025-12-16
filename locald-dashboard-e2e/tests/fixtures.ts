import { test as base } from "@playwright/test";
import { LocaldProcess } from "../src/locald-process";

type MyFixtures = {
  locald: LocaldProcess;
};

export const test = base.extend<MyFixtures>({
  locald: async ({}, use) => {
    const sandboxName = `e2e-${Math.random().toString(36).substring(7)}`;
    const locald = new LocaldProcess(sandboxName);
    await locald.start();
    await use(locald);
    await locald.stop();
  },
  page: async ({ page }, use) => {
    // Anti-Flake Strategy: Inject CSS to freeze animations and hide cursors
    await page.addStyleTag({
      content: `
        *, *::before, *::after {
          animation-play-state: paused !important;
          transition: none !important;
          caret-color: transparent !important; /* Hides native input cursors */
        }
        /* Specific fix for xterm.js cursor blinking */
        .xterm-cursor {
          visibility: hidden !important;
        }
      `,
    });

    // Console Passthrough
    page.on("console", (msg) => console.log(`BROWSER: ${msg.text()}`));

    await use(page);
  },
});

export { expect } from "@playwright/test";
