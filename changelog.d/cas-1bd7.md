### Fixed — Factory

- **Rejected train entries no longer hold siblings** — Was: a rejected candidate stopped the active train and callers could not append deliveries. Now: the rejected task stays parked for repair, the queue advances, and active runs accept appended tasks in order.
