#[cfg(test)]
mod tests {
    use wiremock::matchers::path;

    fn pull_path_matcher() {
        let _matcher = path("/api/sync/pull");
    }
}
