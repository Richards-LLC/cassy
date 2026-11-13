### Fixed — Factory

- **Mail sent in the same millisecond as a tool-boundary check is no longer
  lost** — Was: a message whose inbox signal shared the millisecond of the
  worker's last tool-boundary check stayed hidden until unrelated mail
  arrived. Now: an equal signal triggers one more check, and already shown
  mail is never repeated.
