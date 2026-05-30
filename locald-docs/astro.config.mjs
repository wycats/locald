// @ts-check
import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";

// https://astro.build/config
export default defineConfig({
  site: "http://docs.localhost",
  integrations: [
    starlight({
      title: "locald",
      customCss: ["./src/styles/locald-theme.css", "./src/styles/custom.css"],
      social: [
        {
          label: "GitHub",
          icon: "github",
          href: "https://github.com/ykatz/dotlocal",
        },
      ],
      sidebar: [
        {
          label: "Start Here",
          items: [
            { label: "Overview", link: "/" },
            { label: "Your First Workspace", link: "/getting-started/" },
          ],
        },
        {
          label: "Style Guide",
          items: [
            { label: "Overview", link: "/style-guide/overview/" },
            {
              label: "Brand Foundations",
              link: "/style-guide/brand-foundations/",
            },
            {
              label: "Components & Interface Patterns",
              link: "/style-guide/components-interface-patterns/",
            },
            {
              label: "Product Surface Grammar",
              link: "/style-guide/product-surface-grammar/",
            },
          ],
        },
        {
          label: "App Builder",
          items: [
            {
              label: "Configuration Basics",
              link: "/guides/configuration-basics/",
            },
            { label: "Managed Services", link: "/guides/managed-services/" },
            { label: "Backend Services", link: "/guides/backend-services/" },
            {
              label: "Frontend Development",
              link: "/guides/frontend-development/",
            },
            { label: "DNS & Domains", link: "/guides/dns-and-domains/" },
            { label: "Ad-hoc Tasks", link: "/guides/adhoc-tasks/" },
          ],
        },
        {
          label: "System Tweaker",
          items: [
            { label: "locald.toml", link: "/reference/locald-toml/" },
            { label: "CLI", link: "/reference/cli/" },
            { label: "Integrations", link: "/reference/integrations/" },
            { label: "Service Types", link: "/reference/service-types/" },
            { label: "Execution Modes", link: "/reference/execution-modes/" },
            { label: "Health Checks", link: "/reference/health-checks/" },
          ],
        },
        {
          label: "Experimental",
          collapsed: true,
          items: [
            { label: "Overview", link: "/experimental/" },
            {
              label: "Execution Modes",
              link: "/experimental/execution-modes/",
            },
            { label: "Builds", link: "/experimental/builds/" },
            {
              label: "Ephemeral Containers",
              link: "/experimental/ephemeral-containers/",
            },
          ],
        },
        {
          label: "Roadmap",
          collapsed: true,
          items: [
            { label: "Overview", link: "/roadmap/" },
            { label: "Windows & WSL", link: "/roadmap/windows-and-wsl/" },
            {
              label: "Docker Integration",
              link: "/roadmap/docker-integration/",
            },
            { label: "Advanced Proxying", link: "/roadmap/advanced-proxying/" },
            {
              label: "Constellations & Configuration",
              link: "/roadmap/constellations-and-config/",
            },
            { label: "AI Usability", link: "/roadmap/ai-usability/" },
            {
              label: "Dashboard Ergonomics",
              link: "/roadmap/dashboard-ergonomics/",
            },
            {
              label: "Advanced Service Config",
              link: "/roadmap/advanced-service-config/",
            },
            { label: "UX Improvements", link: "/roadmap/ux-improvements/" },
          ],
        },
        {
          label: "Concepts",
          items: [
            { label: "Philosophy", link: "/concepts/philosophy/" },
            { label: "Vision", link: "/concepts/vision/" },
            { label: "Dashboard Workspace", link: "/concepts/workspace/" },
            { label: "User Personas", link: "/concepts/personas/" },
            {
              label: "Interaction Modes",
              link: "/concepts/user-interaction-modes/",
            },
            { label: "Workflow Axioms", link: "/concepts/workflow-axioms/" },
            {
              label: "Generative Design",
              link: "/concepts/generative-design/",
            },
            { label: "Modes of Collaboration", link: "/concepts/modes/" },
          ],
        },
        {
          label: "Contributor",
          collapsed: true,
          items: [
            {
              label: "Architecture Overview",
              link: "/internals/architecture/",
            },
            {
              label: "Architecture Vision",
              link: "/internals/architecture/vision/",
            },
            { label: "Development Setup", link: "/internals/development/" },
            { label: "Security", link: "/internals/security/" },
            { label: "Design Axioms", link: "/internals/axioms/" },
            { label: "RFCs", link: "/internals/rfcs/" },
          ],
        },
      ],
    }),
  ],
});
