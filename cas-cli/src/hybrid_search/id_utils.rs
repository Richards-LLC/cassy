use regex::Regex;

/// Extract CAS ID patterns from a query string
/// Returns (extracted_ids, remaining_query)
/// Matches patterns like: cas-XXXX, cas-sk0a, rule-041, etc.
pub fn extract_id_patterns(query: &str) -> (Vec<String>, String) {
    // Match CAS-style IDs: cas-XXXX, rule-XXX, cas-skXX, etc.
    // Pattern: word boundary + (cas|rule|skill) + hyphen + alphanumeric
    let re = match Regex::new(r"(?i)\b(cas-[a-z0-9]{2,16}|rule-[a-z0-9]{2,6}|skill-[a-z0-9]{2,6})\b")
    {
        Ok(regex) => regex,
        Err(_) => return (Vec::new(), query.trim().to_string()),
    };

    let mut ids = Vec::new();
    let mut remaining = query.to_string();

    for cap in re.captures_iter(query) {
        if let Some(m) = cap.get(1) {
            ids.push(m.as_str().to_lowercase());
        }
    }

    // Remove matched IDs from query
    remaining = re.replace_all(&remaining, "").to_string();

    // Clean up extra whitespace
    remaining = remaining.split_whitespace().collect::<Vec<_>>().join(" ");

    (ids, remaining)
}

/// Simple glob pattern matching for file paths
/// Supports * (any chars except /) and ** (any chars including /)
pub(crate) fn path_matches_pattern(path: &str, pattern: &str) -> bool {
    // Convert glob pattern to regex
    let mut regex_pattern = String::from("^");
    let mut chars = pattern.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '*' => {
                if chars.peek() == Some(&'*') {
                    chars.next(); // consume second *
                    // Skip optional trailing /
                    if chars.peek() == Some(&'/') {
                        chars.next();
                    }
                    regex_pattern.push_str(".*");
                } else {
                    regex_pattern.push_str("[^/]*");
                }
            }
            '?' => regex_pattern.push('.'),
            '.' | '+' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|' | '\\' => {
                regex_pattern.push('\\');
                regex_pattern.push(c);
            }
            _ => regex_pattern.push(c),
        }
    }
    regex_pattern.push('$');

    Regex::new(&regex_pattern)
        .map(|re| re.is_match(path))
        .unwrap_or(false)
}
