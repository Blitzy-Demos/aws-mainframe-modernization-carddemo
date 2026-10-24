-- COTRTLIC cursor C-TR-TYPE-FORWARD (app/app-transaction-type-db2/cbl/COTRTLIC.cbl:339-352) over the
-- Db2 data version. A NULL filter parameter means the COBOL edit flag is off. ESCAPE '' keeps Db2 LIKE
-- semantics (no escape character).
-- Endpoints 24 and 25 (transaction-types) [A]. Rule: Bounded Pagination (served by xtran_type).
-- Audit: COTRTLIC-004 (forward read), COTRTLIC-005 (filter predicates).
-- Bounded Pagination with a description filter: COTRTLIC always builds the pattern as '%', the
-- trimmed filter text, '%' (COTRTLIC.cbl:1155-1163). A pattern that starts with '%' is never a
-- B-tree index condition, so xtran_type, the table's only index (the Db2 DDL key), serves every
-- page by positioning at the start code in tr_type order while the pattern is applied as a filter.
-- With sequential scans disabled, EXPLAIN (GENERIC_PLAN) plans the query on that index. By cost the
-- planner may instead choose a sequential scan plus a sort on a small table. Either plan reads at
-- most the whole table, which the two-character key keeps small: two-digit codes, the only kind
-- COTRTUPC stores, allow 100 rows, and two printable ASCII characters allow 9,025. A trigram or
-- other description index would add DDL beyond the Db2 data version's declared keys, so the scan
-- is accepted rather than indexed.
-- Parameters: 1 start type code (inclusive), 2 type filter or NULL, 3 description pattern or NULL, 4 limit.
SELECT
    tr_type,
    tr_description
FROM db2.transaction_type
WHERE tr_type >= $1::bpchar
    AND ($2::bpchar IS NULL OR tr_type = $2::bpchar)
    AND ($3::varchar IS NULL OR tr_description LIKE TRIM($3::varchar) ESCAPE '')
ORDER BY tr_type ASC
LIMIT $4::bigint
