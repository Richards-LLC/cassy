## Core Concepts

Test-shape lint no longer treats a Rust loop variable that iterates only string literals (`for source in ["a", "b"]`, with no other binding in the file) as source text. A name bound to file text, or any shell or other-language source comparison, keeps the existing detection.
