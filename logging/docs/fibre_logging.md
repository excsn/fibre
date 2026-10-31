# Fibre Logging Help

## How to Use This Schema in VS Code

To get intelligent autocompletion, validation and hover-documentation for your `fibre_logging.yaml` files in Visual Studio Code, follow these steps:

1.  **Install the YAML Extension**
    
    Make sure you have the official [YAML Language Support by Red Hat](https://marketplace.visualstudio.com/items?itemName=redhat.vscode-yaml) extension installed.

2.  **Save the Schema File**
    
    Copy `docs/fibre_logging.schema.yaml` from the repository into your project or a central schema directory.

3.  **Associate the Schema with Your Config Files**
    
    Open your user or workspace `settings.json` file in VS Code (you can open it via the command palette with `Preferences: Open User Settings (JSON)`). Add the following configuration, adjusting the path to your schema file as needed:
    
    ```json
    "yaml.schemas": {
      "/path/to/your/project/fibre_logging.schema.yaml": [
        "fibre_logging.yaml",
        "fibre_logging.*.yaml" // This glob pattern matches files like fibre_logging.dev.yaml
      ]
    }
    ```
    
    *   **Note**: If you place the schema file in your project's root, you can use a relative path like `./fibre_logging.schema.yaml`.

That's it! Now, when you open any file matching the patterns (e.g., `fibre_logging.yaml`), you will get:

*   **✅ Autocompletion**: Press `Ctrl+Space` to see available properties and their allowed values.
*   **🔍 Validation**: Incorrect property names, data types or enum values will be underlined with an error message.
*   **📖 Documentation**: Hovering your mouse over a property will show its description from the schema.
