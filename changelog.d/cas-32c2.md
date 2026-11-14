### Fixed — Factory

- **Verifier launches survive a supervisor handoff** — A review whose owner
  retired can launch under the live supervisor that claimed its recorded
  handoff, including across factory restarts. Recovery preserves the exact
  review and audits its former owner. Live verifier claims remain protected;
  unresolved ownership asks for repair before suggesting another merge.
