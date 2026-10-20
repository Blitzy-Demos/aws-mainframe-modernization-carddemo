-- COTRN00C PROCESS-PAGE-FORWARD (app/cbl/COTRN00C.cbl:279-328): STARTBR (default GTEQ, 591-619)
-- + READNEXT (624-653) over TRANSACT by TRAN-ID.
-- Endpoint 11 GET /api/v1/transactions. Rule: Bounded Pagination (served by transactions_pkey).
-- Parameters: 1 start transaction ID (inclusive), 2 limit.
-- Parameter 1 is bpchar, never text: a text operand casts tran_id to text, and the primary key
-- then only orders the scan instead of positioning it at the start ID.
SELECT
    tran_id,
    tran_type_cd,
    tran_cat_cd,
    tran_source,
    tran_desc,
    tran_amt,
    tran_merchant_id,
    tran_merchant_name,
    tran_merchant_city,
    tran_merchant_zip,
    card_num,
    orig_ts,
    orig_ts_form,
    proc_ts,
    proc_ts_form,
    filler
FROM transactions
WHERE tran_id >= $1::bpchar
ORDER BY tran_id ASC
LIMIT $2::bigint
