-- Navigation data for the NAV suite (modern/frontend/e2e/nav.spec.ts) and nothing else. It makes
-- no parity claim: no binding asserts these values, and no test other than NAV relies on them.
--
-- Applied only by modern/scripts/e2e.sh, to the local Compose PostgreSQL, after carddemo-migrate
-- has applied the schema, scripts/dev-up.sh has run the nine fixture loads and carddemo-batch
-- posttran has run, through psql with ON_ERROR_STOP on standard input. Those steps leave
-- pending_auth_summaries and pending_auth_details empty, because no ASCII fixture holds
-- authorizations, yet NAV must select an authorization on COPAU00 (/authorizations) to reach
-- COPAU01 (/authorizations/detail).
--
-- These two rows stand in for the CP00 path, which NAV does not run: halt H6 blocks every CP00
-- reply body, and CP00 writes these tables only after its reply.
--
-- Keys: the card, customer and account of the first cross-reference record,
-- app/data/ASCII/cardxref.txt:1. The loads put that account, customer, card and cross-reference in
-- place, so COPAUS0C's reads for the account succeed. The card expiry (MMYY) is that card's, from
-- app/data/ASCII/carddata.txt:1. The detail key is the one COPAUA0C computes for an authorization
-- at 2022-07-18 10:30:00.000 [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:871-875]:
-- auth_date_9c = 99999 - 22199 (YYDDD) and auth_time_9c = 999999999 - 103000000 (HHMMSS * 1000).
-- The summary counts that one approval with its amount; its limits and balances stay zero.
--
-- No DDL: the tables exist only because carddemo-migrate applied
-- modern/crates/persistence/migrations, the only schema source. Every CHAR(n) literal is written
-- at its full width. The file is one transaction without conflict handling, so applying it a
-- second time fails on the summary primary key and leaves both tables as they were, by design.

BEGIN;

INSERT INTO pending_auth_summaries (
    acct_id, cust_id, auth_status,
    account_status_1, account_status_2, account_status_3, account_status_4, account_status_5,
    credit_limit, cash_limit, credit_balance, cash_balance,
    approved_auth_cnt, declined_auth_cnt, approved_auth_amt, declined_auth_amt,
    filler
) VALUES (
    50, 50, ' ',
    '  ', '  ', '  ', '  ', '  ',
    0.00, 0.00, 0.00, 0.00,
    1, 0, 25.00, 0.00,
    '                                  '
);

INSERT INTO pending_auth_details (
    acct_id, auth_date_9c, auth_time_9c,
    auth_orig_date, auth_orig_time,
    card_num, auth_type, card_expiry_date,
    message_type, message_source, auth_id_code,
    auth_resp_code, auth_resp_reason, processing_code,
    transaction_amt, approved_amt,
    merchant_catagory_code, acqr_country_code, pos_entry_mode,
    merchant_id, merchant_name, merchant_city, merchant_state, merchant_zip,
    transaction_id, match_status, auth_fraud, fraud_rpt_date,
    filler
) VALUES (
    50, 77800, 896999999,
    '220718', '103000',
    '0500024453765740', 'SALE', '0323',
    '0100  ', 'POS   ', '000001',
    '00', '0000', 0,
    25.00, 25.00,
    '5411', '840', 1,
    'MERCHANT0000001', 'CARDDEMO NAV MERCHANT ', 'SEATTLE      ', 'WA', '98101    ',
    'NAV000000000001', 'P', ' ', '        ',
    '                 '
);

COMMIT;
