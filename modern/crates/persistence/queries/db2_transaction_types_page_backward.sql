-- COTRTLIC cursor C-TR-TYPE-BACKWARD (app/app-transaction-type-db2/cbl/COTRTLIC.cbl:355-367) over the
-- Db2 data version. The start key is exclusive, as the cursor declares. A NULL filter parameter means
-- the COBOL edit flag is off. ESCAPE '' keeps Db2 LIKE semantics (no escape character).
-- Endpoints 24 and 25 (transaction-types, PF7) [A]. Rule: Bounded Pagination (served by xtran_type).
-- Audit: COTRTLIC-011 (backward read), COTRTLIC-012 (short backward read), COTRTLIC-005 (filter
-- predicates).
-- Bounded Pagination with a description filter: as for the forward page, COTRTLIC always builds the
-- pattern as '%', the trimmed filter text, '%' (COTRTLIC.cbl:1155-1163). A pattern that starts with
-- '%' is never a B-tree index condition, so xtran_type, the table's only index (the Db2 DDL key),
-- serves every page by positioning below the start code and reading in descending tr_type order
-- while the pattern is applied as a filter. With sequential scans disabled, EXPLAIN (GENERIC_PLAN)
-- plans the query on that index: a bitmap scan plus a sort on a freshly migrated table, a backward
-- index scan once it has statistics. By cost the planner may instead choose a sequential scan plus
-- a sort on a small table. Either plan reads at most the whole table, which the two-character key
-- keeps small, as db2_transaction_types_page_forward.sql states.
-- Parameters: 1 start type code (exclusive), 2 type filter or NULL, 3 description pattern or NULL, 4 limit.
SELECT
    tr_type,
    tr_description
FROM db2.transaction_type
WHERE tr_type < $1::bpchar
    AND ($2::bpchar IS NULL OR tr_type = $2::bpchar)
    AND ($3::varchar IS NULL OR tr_description LIKE TRIM($3::varchar) ESCAPE '')
ORDER BY tr_type DESC
LIMIT $4::bigint
