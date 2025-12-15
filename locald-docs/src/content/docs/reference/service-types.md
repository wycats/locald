---
title: Process Types & Start Commands
---

`locald` offers flexible ways to define how your application starts. It attempts to determine the correct start command automatically, but you can always override it.

## Precedence

`locald` determines the start command using the following order of precedence:

1.  **Explicit Command**: A `command` defined in `locald.toml`.
2.  **Procfile**: A `web` process defined in a `Procfile`.
3.  **CNB Metadata**: The default process type defined by the Cloud Native Buildpack (CNB) image.

## 1. Explicit Command

You can explicitly define the command in your `locald.toml`. This is the most direct way to control execution.

```toml
[service.web]
command = "npm run dev"
```

## 2. Procfile

If no explicit command is set, `locald` looks for a `Procfile` in your project root. This is a standard format used by Heroku and other platforms.

```text
web: npm start
worker: npm run worker
```

`locald` will use the command associated with the `web` process type by default.

## 3. CNB Default Process (Container Mode Only)

If you have opted into **Container Execution** (via `[service.build]`) and haven't defined a command or `Procfile`, `locald` will inspect the built image's metadata.

Buildpacks often detect the project type and define a default start command. For example, a Node.js buildpack might default to `node server.js` or `npm start`.

`locald` reads the `io.buildpacks.build.metadata` label from the image and executes the process marked as `default` (or the `web` process if no default is marked).

### How it works

When `locald` starts a CNB-built container without an explicit command:

1.  It reads the OCI image labels.
2.  It parses the `io.buildpacks.build.metadata` JSON.
3.  It finds the process where `default: true` or `type: "web"`.
4.  It constructs the command line to invoke the CNB launcher with that process's command and arguments.

This ensures that "Zero Config" projects work exactly as the buildpack author intended.
