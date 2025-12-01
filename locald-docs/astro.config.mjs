// @ts-check
import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";

// https://astro.build/config
export default defineConfig({
  integrations: [
    starlight({
      title: "locald",
      social: [
        {
          label: "GitHub",
          icon: "github",
          href: "https://github.com/ykatz/dotlocal",
        },
      ],
      sidebar: [
        {
          label: "Guides",
          autogenerate: { directory: "guides" },
        },
        {
          label: "Concepts",
          autogenerate: { directory: "concepts" },
        },
        {
          label: "Reference",
          autogenerate: { directory: "reference" },
        },
        {
          label: "Internals",
          autogenerate: { directory: "internals" },
        },
      ],
    }),
  ],
});
