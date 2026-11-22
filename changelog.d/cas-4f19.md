### Fixed — Commander

- **Read messages stay out of later pane turns** — Before using a prepared transport claim, the relay checks whether an inbox poll or explicit acknowledgement already handled that message. A successor no longer types the same Commander message after reading it through the inbox. Unread messages retain their delivery path and original confirmation provenance.
