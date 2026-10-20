# Hub service binary drift and startup

## First two lines

An update refuses to restart a service pointing at a different binary and names `cas hub service install`; hub status names a running hub’s version drift with the same repair command; stopped hubs retain their start remedy.

## Scannable

Keep the existing service summary and version row; plain service status preserves starting, stopping and unresponsive states, including a starting lock before the runtime record exists.

## Readable

The refusal includes the configured binary and the installed binary so the operator can identify an old candidate without inspecting manager files.

## Machine output

Service reports retain `hub_running` and add `hub_state` (`running`, `starting`, `stopping`, `unresponsive`, `not_running`); hub status adds nullable `version_warning` alongside the existing record and binary version; it is null unless a ready running hub has a different version. JSON stays one document on stdout.

## Omitted

No extra service-manager probes, retries or changes to the live Commander service are required to detect binary drift.
