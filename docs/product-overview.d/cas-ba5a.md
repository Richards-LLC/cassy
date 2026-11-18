## Core Concepts

Test-shape lint no longer treats a Rust loop variable that iterates only string literals (`for source in ["a", "b"]`) as source text inside that loop's body. Code outside the body, a body that rebinds the name, and any shell or other-language source comparison keep the existing detection.
