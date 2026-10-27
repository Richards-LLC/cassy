# Hosted Commander chunk layout

Status: owner validation pending; hosted deployment **NOT EXERCISED** here.

The supervisor reports that the `cas-hub-static` Vercel project in
`petra-stella-cloud/hub-static` receives `hub-web/dist` under
`public/commander/`. Preserve the complete dist-root file set when copying:
`chunk-[name]-[hash].js` and `.css` then resolve at `/commander/chunk-*`.
The exact allowlist is `^chunk-[A-Za-z0-9_-]+-[A-Za-z0-9_-]{8}\.(js|css)$`.
Only these hashed files receive immutable caching; index/app assets retain
revalidation so a deployment can discover the new hashes.

The Cloud owner should inspect `hub-static/scripts/verify-dist.sh` for a
fixed file list and permit this pattern if needed, then verify a JavaScript
chunk and a CSS chunk return their extension's MIME type at the hosted URL.
No external repository, deployment or cache configuration was changed by this
Cassy delivery. This records the layout contract, not a live-host PASS.
