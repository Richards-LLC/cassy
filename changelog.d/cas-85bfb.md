### Added — Commander

- **Screenshots inline in supervisor replies** — Was: a supervisor could not
  show a delivery's screenshots in Commander; Markdown images rendered as text.
  Now: `![alt](artifact:<id>)` or `![alt](<path>)` under the project's artifacts
  directory renders inline, sized for the thread and enlargeable on tap. The
  machine re-proves each image (PNG, JPEG, WebP or GIF, at most 10 MiB, no
  symlinks or paths outside that directory) through the session's media
  ticket; remote URLs and anything refused stay text.
