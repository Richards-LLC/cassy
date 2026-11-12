### Fixed — Factory

- **Train merges finish over landed deliveries** — Was: a train merge whose
  deliveries were already merged or closed stayed at 0 of N and held every later
  single merge on the epic. Now: resuming it, or the next daemon pass, records
  those entries as processed and releases the epic once the list is done.
