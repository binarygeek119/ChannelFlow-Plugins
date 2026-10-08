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

## Releasing a plugin

Each plugin is a `cdylib` (and an `rlib`, so the base can still depend on it by
git). Releases are driven by tags, one plugin per tag:

```
<plugin>-v<version>      e.g.  ai-v2.0.1
```

`<plugin>` is the directory name under `plugins/`. Push the tag and
[`.github/workflows/release-plugin.yml`](.github/workflows/release-plugin.yml)
builds **only that plugin**, then attaches a zip per platform to the GitHub
release for the tag:

```
com.channelflow.ai-v2.0.1-linux-x86_64.zip     libchannelflow_plugin_ai.so + plugin.json
com.channelflow.ai-v2.0.1-windows-x86_64.zip   channelflow_plugin_ai.dll + plugin.json
```

The version in the tag has to match both `plugins/<plugin>/Cargo.toml` and
`plugins/<plugin>/plugin.json`; the workflow refuses to build if they disagree,
so a release cannot drift from the source. To cut one, bump both versions,
commit, then:

```bash
git tag ai-v2.0.1
git push origin ai-v2.0.1
```

Every other push and pull request runs
[`.github/workflows/check.yml`](.github/workflows/check.yml), which builds the
whole workspace and runs the tests. Add a plugin by dropping a crate under
`plugins/` and listing it in the root `Cargo.toml`; the tag `ai-v…` becomes
`<newdir>-v…` and no workflow change is needed.