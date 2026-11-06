### Added — MCP

- **Keep compatible MCP connections through an update** — New Unix connections use a stable stdio front that prepares and checks replacement backends before switching at an idle boundary. Failed readiness, incompatible protocol or tool schemas, and a drain timeout keep the old backend serving. Version reports identify the serving backend separately from the front; existing sessions need a normal harness reconnect to adopt the new transport.
