# Fibre Logging Help

## How to Use This Schema in VS Code

To get intelligent autocompletion, validation, and hover-documentation for your `fibre_logging.yaml` files in Visual Studio Code, follow these steps:

1.  **Install the YAML Extension**
    
    Make sure you have the official [YAML Language Support by Red Hat](https://marketplace.visualstudio.com/items?itemName=redhat.vscode-yaml) extension installed.

2.  **Save the Schema File**
    
    Save the schema content provided above into a file named `fibre_logging.schema.json` in the root of your project or in a central schema directory.

3.  **Associate the Schema with Your Config Files**
    
    Open your user or workspace `settings.json` file in VS Code (you can open it via the command palette with `Preferences: Open User Settings (JSON)`). Add the following configuration, adjusting the path to your schema file as needed:
    
    ```json
    "yaml.schemas": {
      "/path/to/your/project/fibre_logging.schema.json": [
        "fibre_logging.yaml",
        "fibre_logging.*.yaml" // This glob pattern matches files like fibre_logging.dev.yaml
      ]
    }
    ```
    
    *   **Note**: If you place the schema file in your project's root, you can use a relative path like `./fibre_logging.schema.json`.

That's it! Now, when you open any file matching the patterns (e.g., `fibre_logging.yaml`), you will get:

*   **✅ Autocompletion**: Press `Ctrl+Space` to see available properties and their allowed values.
*   **🔍 Validation**: Incorrect property names, data types, or enum values will be underlined with an error message.
*   **📖 Documentation**: Hovering your mouse over a property will show its description from the schema.

---

## Advanced Configuration Tips

### Using Prefixes to Target Modules

You don't need to list every single module path in your configuration. The keys under the `loggers:` section are treated as **prefixes**, and the system automatically applies the configuration to any log target that starts with that prefix.

**The Rule:** The most specific (longest) matching prefix wins. If no specific logger matches, the `root` logger's configuration is used.

#### Example:

```yaml
loggers:
  # Default for everything else
  root:
    level: info
    appenders: [console]

  # This matches "my_app", "my_app::api", "my_app::db", etc.
  my_app:
    level: debug
    appenders: [file_app]
    additive: false # Prevents my_app logs from also going to the console

  # This quiets a noisy dependency
  hyper:
    level: warn
```

**How it works:**
*   A log from `my_app::api` matches the `my_app` logger and is logged to `file_app` at `DEBUG` level.
*   A log from `hyper::client` matches the `hyper` logger; only `WARN` level logs and higher are processed.
*   A log from another crate like `serde` doesn't match a specific logger, so it uses the `root` configuration and is logged to the `console` at `INFO` level.

### Keeping Your Configuration DRY with YAML Anchors

You often want to apply the same logging level and appender settings to multiple, unrelated libraries (e.g., to quiet them all down). While the library does not support an `OR` operator like `|` in the logger key, you can achieve this cleanly using a standard YAML feature: **Anchors and Aliases**.

1.  **Define an Anchor (`&`):** Create a reusable block of configuration and give it a name.
2.  **Use an Alias (`*`):** Refer to that anchored block wherever you need it.

#### Example:

```yaml
# First, define a reusable block for our dependency settings using an anchor (&)
quiet_dependency_config: &quiet_dependency_config
  level: warn
  appenders: [console]
  additive: false

loggers:
  root:
    level: info
    appenders: [console]

  my_app:
    level: debug
    # ...

  # Now, use an alias (*) to reuse the block for each target.
  # This is clean, readable, and easy to maintain.
  hyper: *quiet_dependency_config
  mio: *quiet_dependency_config
  rustls: *quiet_dependency_config
```

This approach keeps your configuration clean and easy to maintain. If you need to change the level for all dependencies, you only have to do it in one place.