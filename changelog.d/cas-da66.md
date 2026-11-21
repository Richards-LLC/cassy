### Fixed — Release validation

- **macOS operator launch smoke** — Launch smoke commands now run as transient
  launchd jobs with a clean environment and captured command exit status.
  Previously, clearing the environment left the factory process ancestry in
  place and correctly triggered the operator guard. Agent controls still fail
  their launch gates; production ancestry checks remain intact.
