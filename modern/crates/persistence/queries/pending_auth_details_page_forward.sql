-- COPAUS0C GET-AUTHORIZATIONS GNP PAUTDTL1
-- (app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:458-486) under GU PAUTSUM0 (973-976),
-- repositioned by REPOSITION-AUTHORIZATIONS WHERE PAUT9CTS = key (488-519).
-- Twin order = ascending PAUT9CTS (DBPAUTP0.dbd:36-37): inverted date and time keys, newest first.
-- Audit rows COPAUS0C-031, COPAUS0C-041 and COPAUS0C-047 of modern/audit/online/COPAUS0C.md.
-- Endpoint 21 GET /api/v1/accounts/{acct_id}/authorizations. Rule: Bounded Pagination
-- (served by pending_auth_details_pkey). Forward only.
-- Parameters: 1 account ID, 2 start inverted date key, 3 start inverted time key (inclusive),
-- 4 limit.
SELECT
    acct_id,
    auth_date_9c,
    auth_time_9c,
    auth_orig_date,
    auth_orig_time,
    card_num,
    auth_type,
    card_expiry_date,
    message_type,
    message_source,
    auth_id_code,
    auth_resp_code,
    auth_resp_reason,
    processing_code,
    transaction_amt,
    approved_amt,
    merchant_catagory_code,
    acqr_country_code,
    pos_entry_mode,
    merchant_id,
    merchant_name,
    merchant_city,
    merchant_state,
    merchant_zip,
    transaction_id,
    match_status,
    auth_fraud,
    fraud_rpt_date,
    filler
FROM pending_auth_details
WHERE acct_id = $1::bigint
    AND (auth_date_9c, auth_time_9c) >= ($2::bigint, $3::bigint)
ORDER BY auth_date_9c ASC, auth_time_9c ASC
LIMIT $4::bigint
