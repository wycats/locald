# Phase 26: Configuration & Constellations

**Goal**: Manage complexity via structure, persistence, and cascading configuration.

## Tasks

- [ ] **Design Proposal**: Create a detailed RFC/Proposal answering the open questions regarding discovery, naming, and schema.
- [ ] **Global Config**: Implement loading and parsing of `~/.config/locald/config.toml`.
- [ ] **Registry**: Implement a persistent registry (`registry.json`) to track known projects and their "Always Up" state.
- [ ] **Cascading Logic**: Implement the configuration merging logic (Global -> Context -> Project).
- [ ] **CLI Commands**: Add `locald config` commands to view and edit configuration.
- [ ] **Dashboard Updates**: Group services by their "Constellation" or directory structure.
