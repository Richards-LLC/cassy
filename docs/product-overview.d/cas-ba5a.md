## Core Concepts

Test-shape lint recognizes mock API error response inputs even when a scenario selector is named `source`. A narrow exemption covers literals on the exact `"error":` protocol key's line inside `.set_body_json(...)`; the general source-text and prose-pin detector retains its existing behavior, including shell source comparisons.
