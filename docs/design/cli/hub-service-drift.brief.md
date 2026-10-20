# Hub service binary drift and startup

## First two lines

An update refuses to restart a service pointing at a different binary and names `cas hub service install`; hub status names version drift with the same repair command.

## Scannable

Keep the existing service summary and version row; after successful activation, report the hub as starting until its ready record exists.

## Readable

The refusal includes the configured binary and the installed binary so the operator can identify an old candidate without inspecting manager files.

## Machine output

Service reports retain `hub_running` and add `hub_state` (`running`, `starting`, `not_running`); hub status adds nullable `version_warning` alongside the existing record and binary version. JSON stays one document on stdout.

## Omitted

No extra service-manager probes, retries or changes to the live Commander service are required to detect binary drift.
