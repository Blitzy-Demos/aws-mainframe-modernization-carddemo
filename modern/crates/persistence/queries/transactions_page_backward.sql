-- COTRN00C PROCESS-PAGE-BACKWARD (app/cbl/COTRN00C.cbl:333-375): STARTBR (default GTEQ, 591-619)
-- + READPREV (658-687) over TRANSACT by TRAN-ID, descending. A READPREV straight after STARTBR
-- returns the start record, so the read is inclusive. The caller drops that record where the
-- program's skip READPREV does (339-341). Audit rows COTRN00C-023 to COTRN00C-026.
-- Endpoint 11 GET /api/v1/transactions (PF7). Rule: Bounded Pagination (served by transactions_pkey).
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
WHERE tran_id <= $1::bpchar
ORDER BY tran_id DESC
LIMIT $2::bigint
