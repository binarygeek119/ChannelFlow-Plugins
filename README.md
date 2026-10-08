# ChannelFlow-Plugins

Plugins for ChannelFlow 2.0.0, one per workspace crate. A plugin depends only
on the [`channelflow-plugin-api`](https://github.com/binarygeek119/ChannelFlow)
SDK from the base repo, implements the `Plugin` trait, and ships a `plugin.json`
manifest describing itself. The base system loads plugins from a catalog, hands
each its `PluginApi` (namespaced storage, HTTP client, logger), and mounts its
routes under `/api/plugins/{id}`.

## Plugins

| Crate | Plugin id | What it does |
|---|---|---|
| [`plugins/ai`](plugins/ai) | `com.channelflow.ai` | The AI Provider Suite — OpenAI-compatible endpoints, tests, and priority-ordered failover |

## How the base builds against this repo

The base depends on each bundled plugin by git, e.g.:

```toml
channelflow-plugin-ai = { git = "https://github.com/binarygeek119/ChannelFlow-Plugins", branch = "main" }
```

A plugin is deliberately a single crate with no imports from the rest of the
base besides the SDK, so living here vs. in the base workspace is only a matter
of where the crate's directory sits.

## Developing a plugin

```bash
cargo test -p channelflow-plugin-ai
```

The SDK is resolved from the base repo's `2.0.0` branch; Cargo.lock pins the
exact revision it was built against.