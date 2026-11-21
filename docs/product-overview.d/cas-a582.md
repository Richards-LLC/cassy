## Core Concepts

A failed capped Playwright run keeps its evidence. Worker-check runs Playwright with its reports directory under `<artifacts>/<task>/worker-check/`, keeps that directory when the run fails, and names it in the failure. Passing runs remove it.
