-- COTRTLIC cursor C-TR-TYPE-FORWARD (app/app-transaction-type-db2/cbl/COTRTLIC.cbl:339-352) over the
-- Db2 data version. A NULL filter parameter means the COBOL edit flag is off. ESCAPE '' keeps Db2 LIKE
-- semantics (no escape character).
-- Endpoints 24 and 25 (transaction-types) [A]. Rule: Bounded Pagination (served by xtran_type).
-- Audit: COTRTLIC-004 (forward read), COTRTLIC-005 (filter predicates).
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
