-- Reproduce the corpus/input/storage portion of the 2026-08-08 ambient recall study.
-- Run read-only against the isolated benchmark database, never the live CAS store:
--   sqlite3 -readonly <bench-root>/cas.db < docs/spikes/2026-08-08-total-ambient-recall-benchmark.sql

.headers on
.mode csv

SELECT
    count(*) AS commits,
    sum(CASE
        WHEN ltrim(subject) LIKE 'Merge branch%'
          OR ltrim(subject) LIKE 'Merge pull request%'
        THEN 1 ELSE 0 END) AS noise_merges,
    sum(CASE
        WHEN NOT (ltrim(subject) LIKE 'Merge branch%'
               OR ltrim(subject) LIKE 'Merge pull request%')
         AND trim(subject || CASE
             WHEN body IS NOT NULL AND trim(body) <> ''
             THEN char(10) || trim(body) ELSE '' END) <> ''
        THEN 1 ELSE 0 END) AS eligible_commits,
    sum(CASE
        WHEN NOT (ltrim(subject) LIKE 'Merge branch%'
               OR ltrim(subject) LIKE 'Merge pull request%')
        THEN length(CAST(subject || CASE
            WHEN body IS NOT NULL AND trim(body) <> ''
            THEN char(10) || trim(body) ELSE '' END AS blob))
        ELSE 0 END) AS commit_input_bytes
FROM history_commits;

SELECT
    count(*) AS docs,
    sum(CASE
        WHEN trim(coalesce(title, '')) <> '' OR trim(coalesce(body, '')) <> ''
        THEN 1 ELSE 0 END) AS eligible_docs,
    sum(length(CAST(CASE
        WHEN trim(coalesce(title, '')) <> '' AND trim(coalesce(body, '')) <> ''
        THEN trim(title) || char(10) || char(10) || trim(body)
        WHEN trim(coalesce(title, '')) <> '' THEN trim(title)
        ELSE trim(coalesce(body, '')) END AS blob))) AS doc_input_bytes
FROM history_docs;
SELECT
    page_count * page_size AS sqlite_allocated_bytes,
    freelist_count * page_size AS sqlite_free_bytes,
    (page_count - freelist_count) * page_size AS sqlite_used_bytes
FROM pragma_page_count(), pragma_page_size(), pragma_freelist_count();

SELECT
    sum(pending_embedding) AS pending_commits,
    count(*) AS total_commits
FROM history_commits;

SELECT
    sum(pending_embedding) AS pending_docs,
    count(*) AS total_docs
FROM history_docs;
