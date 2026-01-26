# locald doctor --json

```console
$ locald doctor --json
? 1
{
  "strategy": {
    "cgroup_root": "direct",
    "why": "container environment detected (host-only workflow)"
  },
  "mode": "degraded",
  "problems": [
    {
      "id": "environment.container",
      "severity": "critical",
      "status": "fail",
      "summary": "locald does not support running inside containers",
      "details": "Run locald on the host OS. If you need the CLI inside a container, expose the host binary into the container using your container tooling.",
      "remediation": [
        "Run locald on the host OS",
        "If needed, expose the host binary into your container using your container tooling"
      ],
      "evidence": [
        {
          "key": "container.detected",
          "value": "true"
        }
      ],
      "fix": "unsupported_environment"
    }
  ],
  "fixes": [
    {
      "key": "unsupported_environment",
      "summary": "Run locald on the host OS",
      "commands": [
        "locald up",
        "# If you need CLI access inside a container:",
        "Expose the host locald binary into the container"
      ]
    }
  ]
}

```
