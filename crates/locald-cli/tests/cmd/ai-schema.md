# locald ai schema

````console
$ locald ai schema
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "LocaldConfig",
  "description": "Root configuration for a locald project./n/nThis is the primary entry point for parsing `locald.toml`./n/n# Example/n```toml/n[project]/nname = /"my-app/"/n/n[plugins]/nredis = /"https://plugins.locald.dev/redis-plugin-1.0.0.locald-package/"/n/n[services.web]/ncommand = /"npm start/"/n```/n/n```rust/nuse locald_core::config::LocaldConfig;/n/nlet raw = r#/"/n[project]/nname = /"my-app/"/n/n[plugins]/nredis = /"https://plugins.locald.dev/redis-plugin-1.0.0.locald-package/"/n/n[services.web]/ncommand = /"npm start/"/n/"#;/n/nlet config: LocaldConfig = toml::from_str(raw).expect(/"valid locald config/");/nassert_eq!(config.project.name, /"my-app/");/nassert!(config.plugins.contains_key(/"redis/"));/nassert!(config.services.contains_key(/"web/"));/n```",
  "type": "object",
  "properties": {
    "plugins": {
      "description": "Plugin sources for remote or local plugins.",
      "type": "object",
      "additionalProperties": {
        "$ref": "#/$defs/PluginSource"
      }
    },
    "project": {
      "description": "Project-level configuration.",
      "$ref": "#/$defs/ProjectConfig"
    },
    "services": {
      "description": "Service definitions for the project.",
      "type": "object",
      "additionalProperties": {
        "$ref": "#/$defs/ServiceConfig"
      },
      "default": {}
    }
  },
  "required": [
    "project"
  ],
  "$defs": {
    "BuildConfig": {
      "description": "Configuration for building a service using Cloud Native Buildpacks./n/n# Example/n```toml/n[services.web.build]/nbuilder = /"heroku/builder:22/"/nbuildpacks = [/"heroku/nodejs/"]/n```",
      "type": "object",
      "properties": {
        "builder": {
          "description": "The builder image to use. Defaults to /"heroku/builder:22/".",
          "type": "string",
          "default": "heroku/builder:22"
        },
        "buildpacks": {
          "description": "List of buildpacks to use.",
          "type": "array",
          "items": {
            "type": "string"
          }
        }
      }
    },
    "ContainerServiceConfig": {
      "description": "Configuration for a container-based service./n/n# Example/n```toml/n[services.redis]/ntype = /"container/"/nimage = /"redis:7/"/ncontainer_port = 6379/n```",
      "type": "object",
      "properties": {
        "command": {
          "description": "The command to run in the container.",
          "type": [
            "string",
            "null"
          ]
        },
        "container_port": {
          "description": "The port exposed by the container.",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint16",
          "maximum": 65535,
          "minimum": 0
        },
        "depends_on": {
          "description": "List of services that must be started before this one.",
          "type": "array",
          "items": {
            "type": "string"
          }
        },
        "env": {
          "description": "Environment variables to pass to the service.",
          "type": "object",
          "additionalProperties": {
            "type": "string"
          }
        },
        "health_check": {
          "description": "Optional command to run to check if the service is healthy./nIf not provided, locald will attempt to infer a health check (Docker, Notify, or TCP).",
          "anyOf": [
            {
              "$ref": "#/$defs/HealthCheckConfig"
            },
            {
              "type": "null"
            }
          ]
        },
        "image": {
          "description": "The Docker image to run.",
          "type": "string"
        },
        "port": {
          "description": "The port the service listens on. If None, locald will assign a port and pass it via PORT env var.",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint16",
          "maximum": 65535,
          "minimum": 0
        },
        "stop_signal": {
          "description": "The signal to send to stop the service. Defaults to /"SIGTERM/".",
          "type": [
            "string",
            "null"
          ]
        },
        "workdir": {
          "description": "Working directory inside the container.",
          "type": [
            "string",
            "null"
          ]
        }
      },
      "required": [
        "image"
      ]
    },
    "ExecServiceConfig": {
      "description": "Configuration for a generic executable service./n/n# Example/n```toml/ncommand = /"npm start/"/n```",
      "type": "object",
      "properties": {
        "build": {
          "description": "Configuration for building the service using CNB.",
          "anyOf": [
            {
              "$ref": "#/$defs/BuildConfig"
            },
            {
              "type": "null"
            }
          ]
        },
        "command": {
          "description": "The command to run to start the service.",
          "type": [
            "string",
            "null"
          ]
        },
        "depends_on": {
          "description": "List of services that must be started before this one.",
          "type": "array",
          "items": {
            "type": "string"
          }
        },
        "env": {
          "description": "Environment variables to pass to the service.",
          "type": "object",
          "additionalProperties": {
            "type": "string"
          }
        },
        "health_check": {
          "description": "Optional command to run to check if the service is healthy./nIf not provided, locald will attempt to infer a health check (Docker, Notify, or TCP).",
          "anyOf": [
            {
              "$ref": "#/$defs/HealthCheckConfig"
            },
            {
              "type": "null"
            }
          ]
        },
        "port": {
          "description": "The port the service listens on. If None, locald will assign a port and pass it via PORT env var.",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint16",
          "maximum": 65535,
          "minimum": 0
        },
        "stop_signal": {
          "description": "The signal to send to stop the service. Defaults to /"SIGTERM/".",
          "type": [
            "string",
            "null"
          ]
        },
        "workdir": {
          "description": "Working directory for the command. Defaults to the project root.",
          "type": [
            "string",
            "null"
          ]
        }
      }
    },
    "HealthCheckConfig": {
      "description": "Configuration for service health checks./n/n# Example/n```toml/nhealth_check = { type = /"http/", path = /"/health/" }/n# OR/nhealth_check = /"curl -f http://localhost:3000/health/"/n```",
      "anyOf": [
        {
          "description": "A shell command to run.",
          "type": "string"
        },
        {
          "description": "A structured probe configuration.",
          "$ref": "#/$defs/ProbeConfig"
        }
      ]
    },
    "PluginSource": {
      "description": "A plugin source reference in locald.toml./n/n# Example/n```toml/n[plugins]/n# Simple URL reference/nredis = /"https://plugins.locald.dev/redis-plugin-1.0.0.locald-package/"/n/n# URL with checksum verification/npostgres = { url = /"https://plugins.locald.dev/postgres-plugin.locald-package/", sha256 = /"abc123.../" }/n/n# Local path reference (useful for development)/ncustom = { path = /"../my-custom-plugin/target/plugin.wasm/" }/n```",
      "anyOf": [
        {
          "description": "Simple URL string.",
          "type": "string"
        },
        {
          "description": "URL with optional checksum.",
          "type": "object",
          "properties": {
            "sha256": {
              "description": "SHA-256 checksum of the package.",
              "type": [
                "string",
                "null"
              ]
            },
            "url": {
              "description": "The URL to fetch the package from.",
              "type": "string"
            }
          },
          "required": [
            "url"
          ]
        },
        {
          "description": "Local path reference.",
          "type": "object",
          "properties": {
            "path": {
              "description": "Path to the plugin WASM file or package.",
              "type": "string"
            }
          },
          "required": [
            "path"
          ]
        },
        {
          "description": "Reference to an installed plugin (explicit, usually auto-discovered).",
          "type": "object",
          "properties": {
            "installed": {
              "description": "Name of the installed plugin.",
              "type": "string"
            }
          },
          "required": [
            "installed"
          ]
        }
      ]
    },
    "PostgresServiceConfig": {
      "description": "Configuration for a managed Postgres service./n/n# Example/n```toml/ntype = /"postgres/"/nversion = /"15/"/n```",
      "type": "object",
      "properties": {
        "depends_on": {
          "description": "List of services that must be started before this one.",
          "type": "array",
          "items": {
            "type": "string"
          }
        },
        "env": {
          "description": "Environment variables to pass to the service.",
          "type": "object",
          "additionalProperties": {
            "type": "string"
          }
        },
        "health_check": {
          "description": "Optional command to run to check if the service is healthy./nIf not provided, locald will attempt to infer a health check (Docker, Notify, or TCP).",
          "anyOf": [
            {
              "$ref": "#/$defs/HealthCheckConfig"
            },
            {
              "type": "null"
            }
          ]
        },
        "port": {
          "description": "The port the service listens on. If None, locald will assign a port and pass it via PORT env var.",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint16",
          "maximum": 65535,
          "minimum": 0
        },
        "stop_signal": {
          "description": "The signal to send to stop the service. Defaults to /"SIGTERM/".",
          "type": [
            "string",
            "null"
          ]
        },
        "version": {
          "description": "The version of Postgres to use. Defaults to stable.",
          "type": [
            "string",
            "null"
          ]
        }
      }
    },
    "ProbeConfig": {
      "description": "Configuration for a health check probe./n/n# Example/n```toml/ntype = /"http/"/npath = /"/health/"/ninterval = 5/n```",
      "type": "object",
      "properties": {
        "command": {
          "description": "The command to run (for Command probes).",
          "type": [
            "string",
            "null"
          ],
          "default": null
        },
        "interval": {
          "description": "The interval between checks in seconds. Defaults to 1 second.",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint64",
          "default": null,
          "minimum": 0
        },
        "path": {
          "description": "The path to check (for HTTP probes).",
          "type": [
            "string",
            "null"
          ],
          "default": null
        },
        "timeout": {
          "description": "The timeout for each check in seconds. Defaults to 5 seconds.",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint64",
          "default": null,
          "minimum": 0
        },
        "type": {
          "description": "The type of probe to perform.",
          "$ref": "#/$defs/ProbeType"
        }
      },
      "required": [
        "type"
      ]
    },
    "ProbeType": {
      "description": "The type of health check probe./n/n# Example/n```toml/ntype = /"http/"/n```",
      "oneOf": [
        {
          "description": "An HTTP GET request.",
          "type": "string",
          "const": "http"
        },
        {
          "description": "A TCP connection attempt.",
          "type": "string",
          "const": "tcp"
        },
        {
          "description": "A shell command execution.",
          "type": "string",
          "const": "command"
        }
      ]
    },
    "ProjectConfig": {
      "description": "Configuration specific to the project identity./n/nThe `name` is required and influences default domains and identifiers./n/n# Example/n```toml/n[project]/nname = /"my-app/"/ndomain = /"myapp.local/"/n```",
      "type": "object",
      "properties": {
        "constellation": {
          "description": "The name of the constellation the project belongs to.",
          "type": [
            "string",
            "null"
          ]
        },
        "domain": {
          "description": "The domain to serve the project on. Defaults to `{name}.localhost`.",
          "type": [
            "string",
            "null"
          ]
        },
        "name": {
          "description": "The name of the project.",
          "type": "string"
        },
        "workspace": {
          "description": "The name of the workspace the project belongs to.",
          "type": [
            "string",
            "null"
          ]
        }
      },
      "required": [
        "name"
      ]
    },
    "ServiceConfig": {
      "description": "Configuration for a single service./n/nThis enum is untagged, so a service entry can be either a typed service/n(with a `type = /".../"` field) or a legacy exec-style service config./n/n# Example/n```toml/n[services.web]/ncommand = /"npm start/"/n```",
      "anyOf": [
        {
          "description": "A typed service configuration (e.g. Postgres, Worker).",
          "$ref": "#/$defs/TypedServiceConfig"
        },
        {
          "description": "A legacy or simple exec service configuration.",
          "$ref": "#/$defs/ExecServiceConfig"
        }
      ]
    },
    "SiteServiceConfig": {
      "description": "Configuration for a managed site service./n/n# Example/n```toml/n[services.docs]/ntype = /"site/"/npath = /"./docs/"/nbuild = /"cargo doc/"/n```",
      "type": "object",
      "properties": {
        "build": {
          "description": "The command to run to build the site.",
          "type": "string",
          "default": ""
        },
        "depends_on": {
          "description": "List of services that must be started before this one.",
          "type": "array",
          "items": {
            "type": "string"
          }
        },
        "env": {
          "description": "Environment variables to pass to the service.",
          "type": "object",
          "additionalProperties": {
            "type": "string"
          }
        },
        "health_check": {
          "description": "Optional command to run to check if the service is healthy./nIf not provided, locald will attempt to infer a health check (Docker, Notify, or TCP).",
          "anyOf": [
            {
              "$ref": "#/$defs/HealthCheckConfig"
            },
            {
              "type": "null"
            }
          ]
        },
        "path": {
          "description": "The path to the directory to serve.",
          "type": "string"
        },
        "port": {
          "description": "The port the service listens on. If None, locald will assign a port and pass it via PORT env var.",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint16",
          "maximum": 65535,
          "minimum": 0
        },
        "stop_signal": {
          "description": "The signal to send to stop the service. Defaults to /"SIGTERM/".",
          "type": [
            "string",
            "null"
          ]
        }
      },
      "required": [
        "path"
      ]
    },
    "TypedServiceConfig": {
      "description": "Enum of supported typed service configurations./n/n# Example/n```toml/n[services.db]/ntype = /"postgres/"/nversion = /"15/"/n```",
      "oneOf": [
        {
          "description": "A generic executable service.",
          "type": "object",
          "properties": {
            "type": {
              "type": "string",
              "const": "exec"
            }
          },
          "$ref": "#/$defs/ExecServiceConfig",
          "required": [
            "type"
          ]
        },
        {
          "description": "A managed Postgres database service.",
          "type": "object",
          "properties": {
            "type": {
              "type": "string",
              "const": "postgres"
            }
          },
          "$ref": "#/$defs/PostgresServiceConfig",
          "required": [
            "type"
          ]
        },
        {
          "description": "A background worker service.",
          "type": "object",
          "properties": {
            "type": {
              "type": "string",
              "const": "worker"
            }
          },
          "$ref": "#/$defs/WorkerServiceConfig",
          "required": [
            "type"
          ]
        },
        {
          "description": "A container-based service.",
          "type": "object",
          "properties": {
            "type": {
              "type": "string",
              "const": "container"
            }
          },
          "$ref": "#/$defs/ContainerServiceConfig",
          "required": [
            "type"
          ]
        },
        {
          "description": "A managed site service.",
          "type": "object",
          "properties": {
            "type": {
              "type": "string",
              "const": "site"
            }
          },
          "$ref": "#/$defs/SiteServiceConfig",
          "required": [
            "type"
          ]
        }
      ]
    },
    "WorkerServiceConfig": {
      "description": "Configuration for a background worker service./n/n# Example/n```toml/n[services.worker]/ntype = /"worker/"/ncommand = /"bundle exec sidekiq/"/n```",
      "type": "object",
      "properties": {
        "command": {
          "description": "The command to run to start the worker.",
          "type": "string"
        },
        "depends_on": {
          "description": "List of services that must be started before this one.",
          "type": "array",
          "items": {
            "type": "string"
          }
        },
        "env": {
          "description": "Environment variables to pass to the service.",
          "type": "object",
          "additionalProperties": {
            "type": "string"
          }
        },
        "health_check": {
          "description": "Optional command to run to check if the service is healthy./nIf not provided, locald will attempt to infer a health check (Docker, Notify, or TCP).",
          "anyOf": [
            {
              "$ref": "#/$defs/HealthCheckConfig"
            },
            {
              "type": "null"
            }
          ]
        },
        "port": {
          "description": "The port the service listens on. If None, locald will assign a port and pass it via PORT env var.",
          "type": [
            "integer",
            "null"
          ],
          "format": "uint16",
          "maximum": 65535,
          "minimum": 0
        },
        "stop_signal": {
          "description": "The signal to send to stop the service. Defaults to /"SIGTERM/".",
          "type": [
            "string",
            "null"
          ]
        },
        "workdir": {
          "description": "Working directory for the command. Defaults to the project root.",
          "type": [
            "string",
            "null"
          ]
        }
      },
      "required": [
        "command"
      ]
    }
  }
}

````
