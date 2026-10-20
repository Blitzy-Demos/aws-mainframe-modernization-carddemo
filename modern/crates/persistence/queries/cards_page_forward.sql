-- COCRDLIC 9000-READ-FORWARD (app/cbl/COCRDLIC.cbl:1123-1263) and 9500-FILTER-RECORDS (1382-1411):
-- STARTBR GTEQ + READNEXT over CARDDAT by card number, filters applied per record.
-- Endpoint 7 GET /api/v1/cards. Rule: Bounded Pagination (served by cards_pkey).
-- Parameters: 1 start card number (inclusive), 2 account filter or NULL, 3 card filter or NULL, 4 limit.
-- Audit rows COCRDLIC-010, COCRDLIC-017, COCRDLIC-018 and COCRDLIC-023. The look-ahead read of
-- COCRDLIC-019 ignores the filters, so callers issue it with parameters 2 and 3 NULL.
SELECT
    card_num,
    acct_id,
    card_cvv_cd,
    card_embossed_name,
    card_expiraion_date,
    card_active_status,
    filler,
    card_ref
FROM cards
WHERE card_num >= $1::bpchar
    AND ($2::bigint IS NULL OR acct_id = $2::bigint)
    AND ($3::bpchar IS NULL OR card_num = $3::bpchar)
ORDER BY card_num ASC
LIMIT $4::bigint
