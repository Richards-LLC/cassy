## Core Concepts

A task close that outlives its MCP response budget returns progress with a durable continuation ID. The task's final outcome note uses the same identity, so workers can observe a delayed park or refusal without repeating the mutation. Only a checked close or park reports a committed disposition; a parked delivery still awaits its merge verdict.
