-- COCRDLIC 9100-READ-BACKWARDS (app/cbl/COCRDLIC.cbl:1264-1380), 9500-FILTER-RECORDS (1382-1411):
-- STARTBR GTEQ + READPREV over CARDDAT by card number, the first READPREV returning the start
-- record itself (inclusive), filters applied per record.
-- Endpoint 7 GET /api/v1/cards (PF7). Rule: Bounded Pagination (served by cards_pkey).
-- Parameters: 1 start card number (inclusive), 2 account filter or NULL, 3 card filter or NULL, 4 limit.
-- Audit rows COCRDLIC-010, COCRDLIC-027 and COCRDLIC-029. The program discards the start record
-- unfiltered, so callers drop it by key equality (a filter may already have excluded it) and
-- reverse the rows, which arrive in descending card-number order, for display.
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
WHERE card_num <= $1::bpchar
    AND ($2::bigint IS NULL OR acct_id = $2::bigint)
    AND ($3::bpchar IS NULL OR card_num = $3::bpchar)
ORDER BY card_num DESC
LIMIT $4::bigint
