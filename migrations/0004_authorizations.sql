-- Pending authorization schema: the IMS HIDAM database DBPAUTP0, whose root segment PAUTSUM0
-- holds one pending authorization summary per account and whose child segment PAUTDTL1 holds
-- one pending authorization, and the Db2 table CARDDEMO.AUTHFRDS of authorizations reported
-- as fraud. It replaces the DBPAUTP0 and DBPAUTX0 DBDs and the AUTHFRDS DDL.
--
-- All three tables start empty (P-CIPAUSMY-014): `batch seed` loads nothing into them. The
-- IMS unload AWS.M2.CARDDEMO.IMSDATA.DBPAUTP0.dat is not loaded, because importing data
-- beyond the 626 fixture records is out of scope. Rows come only from these programs:
--   CP00, COPAUA0C (crates/messaging/src/flows/cp00.rs), after it acknowledges the request:
--     8400-UPDATE-SUMMARY inserts the summary when it is missing and replaces it otherwise,
--     then 8500-INSERT-AUTH inserts the detail under it
--     [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L798-933] (P-COPAUA0C-013,
--     P-COPAUA0C-014). A failed write is logged at I003 or I004.
--   CPVD fraud marking, COPAUS1C with COPAUS2C (PUT /authorizations/{id}): COPAUS2C inserts
--     into AUTHFRDS and, on SQLCODE -803, updates the existing row
--     [app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl:L141-243] (P-COPAUS2C-003,
--     P-COPAUS2C-004); COPAUS1C then replaces the detail (P-COPAUS1C-008).
--   `batch cbpaup0j`, CBPAUP0C: deletes each expired detail, then deletes a summary whose
--     approved count, as decremented in memory, is at most 0, together with the details
--     still under it [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L156-158]
--     [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L303-347] (P-CBPAUP0C-005 to
--     P-CBPAUP0C-007, E-16). The decrements change only the program's copy of the segment,
--     which it never replaces, so a purge never updates a stored summary (E-35).
--
-- Sources:
--   app/app-authorization-ims-db2-mq/cpy/CIPAUSMY.cpy, CIPAUDTY.cpy    segment layouts
--   app/app-authorization-ims-db2-mq/ims/DBPAUTP0.dbd                  segments, parentage, sequence fields
--   app/app-authorization-ims-db2-mq/ims/DBPAUTX0.dbd                  primary index on ACCNTID
--   app/app-authorization-ims-db2-mq/ddl/AUTHFRDS.ddl, XAUTHFRD.ddl    Db2 table, primary key, unique index
--   app/app-authorization-ims-db2-mq/dcl/AUTHFRDS.dcl                  DCLGEN host variables
--   app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl, COPAUS2C.cbl, CBPAUP0C.cbl
--                                                                      writers and purge
--   docs/parity/parity-oracle-audit.md, section A                      reviewed layout rows cited below
--
-- Conventions (shared with 0001 to 0003):
--   X(n), Db2 CHAR(n) and Db2 VARCHAR(n) map to VARCHAR(n) COLLATE "C". The database default
--   collation (en_US.utf8 in postgres:16.15) does not sort by byte value; "C" gives the byte
--   order of COBOL comparisons and of the Db2 index on CARD_NUM.
--   Text is stored right-trimmed of trailing spaces; persistence pads it back to the PIC
--   width.
--   Unsigned 9(n) maps to INTEGER for n <= 9 with CHECK (0 <= v < 10^n). A signed binary or
--   packed integer S9(n) maps to INTEGER with CHECK (-10^n < v < 10^n), unless its audit row
--   names another type (below).
--   S9(n)V99 maps to NUMERIC(n+2,2) whatever its USAGE: COMP-3 is read by its PIC digits, so
--   the precision bounds the value to the n integer digits and the scale to the 2 decimals.
--   Db2 DECIMAL(p,s) maps to NUMERIC(p,s), TIMESTAMP to TIMESTAMP(6) without time zone, DATE
--   to DATE and SMALLINT to SMALLINT.
--   Every IMS segment field is NOT NULL, as every byte of a segment holds a value. The Db2
--   columns keep their DDL nullability.
--   Text columns carry no value-domain check: the 88-level values of PA-MATCH-STATUS (P, D,
--   E, M) and PA-AUTH-FRAUD (F, R) are not enforced, and COPAUA0C stores the fraud fields as
--   spaces [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L908-909].
--   FILLER is not stored.
--   No column defaults to the current time: writers supply the processing clock
--   (CARDDEMO_CLOCK_OVERRIDE when set), FRAUD_RPT_DATE included, which COPAUS2C sets to the
--   Db2 CURRENT DATE [app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl:L194].
--   IMS fields are named by their copybook names lower-snake-cased, prefix and spelling kept
--   (pa_merchant_catagory_code). The IMS key columns keep their copybook names too, because
--   the request's key names cover only entity primary keys. Db2 columns keep their DDL names
--   lower-cased.
--
-- Reviewed audit rows that decide a column against the general rules above:
--   P-CIPAUSMY-001       pa_acct_id is BIGINT CHECK (0 <= v < 10^11), the AccountId range,
--                        although PA-ACCT-ID is signed: COPAUA0C fills it only from the
--                        unsigned XREF-ACCT-ID 9(11)
--                        [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L805]
--                        [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L911]. The
--                        detail's pa_acct_id carries the same check.
--   P-CIPAUSMY-004       PA-ACCOUNT-STATUS X(02) OCCURS 5 TIMES is the one column
--                        pa_account_status VARCHAR(2)[5]: occurrence i is subscript i. A check
--                        holds the array to one dimension of exactly 5 non-null elements
--                        from subscript 1, because PostgreSQL does not enforce a declared
--                        array size.
--   P-CIPAUSMY-009, -010 the S9(04) COMP counts are SMALLINT, the binary halfword the
--                        AuthCount newtype holds, with CHECK (-10^4 < v < 10^4) for the four
--                        PIC digits.
--   P-AUTHFRDS-025, -026 ACCT_ID DECIMAL(11) and CUST_ID DECIMAL(9) are BIGINT with
--                        CHECK (0 <= v < 10^11) and CHECK (0 <= v < 10^9), the AccountId and
--                        CustomerId ranges: COPAUS2C fills them from the unsigned WS-ACCT-ID
--                        9(11) and WS-CUST-ID 9(9)
--                        [app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl:L75-76].
--
-- pending_auth_summaries: IMS DBPAUTP0 root segment PAUTSUM0, CIPAUSMY PENDING-AUTH-SUMMARY,
-- BYTES=100, sequence field ACCNTID SEQ,U packed 6 bytes at offset 0
-- [app/app-authorization-ims-db2-mq/ims/DBPAUTP0.dbd:L28-30]
--   column               | copybook field       | PIC (USAGE)            | SQL type                              | audit row
--   pa_acct_id           | PA-ACCT-ID           | S9(11) COMP-3          | BIGINT NOT NULL, 0 to 99999999999     | P-CIPAUSMY-001
--   pa_cust_id           | PA-CUST-ID           | 9(09)                  | INTEGER NOT NULL, 0 to 999999999      | P-CIPAUSMY-002
--   pa_auth_status       | PA-AUTH-STATUS       | X(01)                  | VARCHAR(1) COLLATE "C" NOT NULL       | P-CIPAUSMY-003
--   pa_account_status    | PA-ACCOUNT-STATUS    | X(02) OCCURS 5 TIMES   | VARCHAR(2)[5] COLLATE "C" NOT NULL    | P-CIPAUSMY-004
--   pa_credit_limit      | PA-CREDIT-LIMIT      | S9(09)V99 COMP-3       | NUMERIC(11,2) NOT NULL                | P-CIPAUSMY-005
--   pa_cash_limit        | PA-CASH-LIMIT        | S9(09)V99 COMP-3       | NUMERIC(11,2) NOT NULL                | P-CIPAUSMY-006
--   pa_credit_balance    | PA-CREDIT-BALANCE    | S9(09)V99 COMP-3       | NUMERIC(11,2) NOT NULL                | P-CIPAUSMY-007
--   pa_cash_balance      | PA-CASH-BALANCE      | S9(09)V99 COMP-3       | NUMERIC(11,2) NOT NULL                | P-CIPAUSMY-008
--   pa_approved_auth_cnt | PA-APPROVED-AUTH-CNT | S9(04) COMP            | SMALLINT NOT NULL, -9999 to 9999      | P-CIPAUSMY-009
--   pa_declined_auth_cnt | PA-DECLINED-AUTH-CNT | S9(04) COMP            | SMALLINT NOT NULL, -9999 to 9999      | P-CIPAUSMY-010
--   pa_approved_auth_amt | PA-APPROVED-AUTH-AMT | S9(09)V99 COMP-3       | NUMERIC(11,2) NOT NULL                | P-CIPAUSMY-011
--   pa_declined_auth_amt | PA-DECLINED-AUTH-AMT | S9(09)V99 COMP-3       | NUMERIC(11,2) NOT NULL                | P-CIPAUSMY-012
--   (not stored)         | FILLER               | X(34)                  | not stored                            | P-CIPAUSMY-013
--
-- pending_auth_details: IMS DBPAUTP0 child segment PAUTDTL1 under PAUTSUM0, CIPAUDTY
-- PENDING-AUTH-DETAILS, BYTES=200, sequence field PAUT9CTS SEQ,U 8 bytes at offset 0, the
-- packed PA-AUTH-DATE-9C and PA-AUTH-TIME-9C
-- [app/app-authorization-ims-db2-mq/ims/DBPAUTP0.dbd:L36-37]
--   column                    | copybook field            | PIC (USAGE)      | SQL type                                  | audit row
--   auth_id                   | (none)                    | (none)           | BIGINT NOT NULL, identity, primary key    | new (REST key)
--   pa_acct_id                | PA-ACCT-ID of the parent  | S9(11) COMP-3    | BIGINT NOT NULL, 0 to 99999999999         | P-CIPAUSMY-001
--   pa_auth_date_9c           | PA-AUTH-DATE-9C           | S9(05) COMP-3    | INTEGER NOT NULL, -99999 to 99999         | P-CIPAUDTY-001
--   pa_auth_time_9c           | PA-AUTH-TIME-9C           | S9(09) COMP-3    | INTEGER NOT NULL, -999999999 to 999999999 | P-CIPAUDTY-002
--   pa_auth_orig_date         | PA-AUTH-ORIG-DATE         | X(06)            | VARCHAR(6) COLLATE "C" NOT NULL           | P-CIPAUDTY-003
--   pa_auth_orig_time         | PA-AUTH-ORIG-TIME         | X(06)            | VARCHAR(6) COLLATE "C" NOT NULL           | P-CIPAUDTY-004
--   pa_card_num               | PA-CARD-NUM               | X(16)            | VARCHAR(16) COLLATE "C" NOT NULL          | P-CIPAUDTY-005
--   pa_auth_type              | PA-AUTH-TYPE              | X(04)            | VARCHAR(4) COLLATE "C" NOT NULL           | P-CIPAUDTY-006
--   pa_card_expiry_date       | PA-CARD-EXPIRY-DATE       | X(04)            | VARCHAR(4) COLLATE "C" NOT NULL           | P-CIPAUDTY-007
--   pa_message_type           | PA-MESSAGE-TYPE           | X(06)            | VARCHAR(6) COLLATE "C" NOT NULL           | P-CIPAUDTY-008
--   pa_message_source         | PA-MESSAGE-SOURCE         | X(06)            | VARCHAR(6) COLLATE "C" NOT NULL           | P-CIPAUDTY-009
--   pa_auth_id_code           | PA-AUTH-ID-CODE           | X(06)            | VARCHAR(6) COLLATE "C" NOT NULL           | P-CIPAUDTY-010
--   pa_auth_resp_code         | PA-AUTH-RESP-CODE         | X(02)            | VARCHAR(2) COLLATE "C" NOT NULL           | P-CIPAUDTY-011
--   pa_auth_resp_reason       | PA-AUTH-RESP-REASON       | X(04)            | VARCHAR(4) COLLATE "C" NOT NULL           | P-CIPAUDTY-012
--   pa_processing_code        | PA-PROCESSING-CODE        | 9(06)            | INTEGER NOT NULL, 0 to 999999             | P-CIPAUDTY-013
--   pa_transaction_amt        | PA-TRANSACTION-AMT        | S9(10)V99 COMP-3 | NUMERIC(12,2) NOT NULL                    | P-CIPAUDTY-014
--   pa_approved_amt           | PA-APPROVED-AMT           | S9(10)V99 COMP-3 | NUMERIC(12,2) NOT NULL                    | P-CIPAUDTY-015
--   pa_merchant_catagory_code | PA-MERCHANT-CATAGORY-CODE | X(04)            | VARCHAR(4) COLLATE "C" NOT NULL           | P-CIPAUDTY-016
--   pa_acqr_country_code      | PA-ACQR-COUNTRY-CODE      | X(03)            | VARCHAR(3) COLLATE "C" NOT NULL           | P-CIPAUDTY-017
--   pa_pos_entry_mode         | PA-POS-ENTRY-MODE         | 9(02)            | INTEGER NOT NULL, 0 to 99                 | P-CIPAUDTY-018
--   pa_merchant_id            | PA-MERCHANT-ID            | X(15)            | VARCHAR(15) COLLATE "C" NOT NULL          | P-CIPAUDTY-019
--   pa_merchant_name          | PA-MERCHANT-NAME          | X(22)            | VARCHAR(22) COLLATE "C" NOT NULL          | P-CIPAUDTY-020
--   pa_merchant_city          | PA-MERCHANT-CITY          | X(13)            | VARCHAR(13) COLLATE "C" NOT NULL          | P-CIPAUDTY-021
--   pa_merchant_state         | PA-MERCHANT-STATE         | X(02)            | VARCHAR(2) COLLATE "C" NOT NULL           | P-CIPAUDTY-022
--   pa_merchant_zip           | PA-MERCHANT-ZIP           | X(09)            | VARCHAR(9) COLLATE "C" NOT NULL           | P-CIPAUDTY-023
--   pa_transaction_id         | PA-TRANSACTION-ID         | X(15)            | VARCHAR(15) COLLATE "C" NOT NULL          | P-CIPAUDTY-024
--   pa_match_status           | PA-MATCH-STATUS           | X(01)            | VARCHAR(1) COLLATE "C" NOT NULL           | P-CIPAUDTY-025
--   pa_auth_fraud             | PA-AUTH-FRAUD             | X(01)            | VARCHAR(1) COLLATE "C" NOT NULL           | P-CIPAUDTY-026
--   pa_fraud_rpt_date         | PA-FRAUD-RPT-DATE         | X(08)            | VARCHAR(8) COLLATE "C" NOT NULL           | P-CIPAUDTY-027
--   (not stored)              | FILLER                    | X(17)            | not stored                                | P-CIPAUDTY-028
--   auth_id is generated and never reused; GET and PUT /authorizations/{id} address a detail
--   by it, while the IMS key columns stay alongside it unchanged.
--
-- auth_fraud: Db2 CARDDEMO.AUTHFRDS [app/app-authorization-ims-db2-mq/ddl/AUTHFRDS.ddl:L1-28]
--   column                 | DDL field              | DDL type           | SQL type                          | audit row
--   card_num               | CARD_NUM               | CHAR(16) NOT NULL  | VARCHAR(16) COLLATE "C" NOT NULL  | P-AUTHFRDS-001
--   auth_ts                | AUTH_TS                | TIMESTAMP NOT NULL | TIMESTAMP(6) NOT NULL             | P-AUTHFRDS-002
--   auth_type              | AUTH_TYPE              | CHAR(4)            | VARCHAR(4) COLLATE "C"            | P-AUTHFRDS-003
--   card_expiry_date       | CARD_EXPIRY_DATE       | CHAR(4)            | VARCHAR(4) COLLATE "C"            | P-AUTHFRDS-004
--   message_type           | MESSAGE_TYPE           | CHAR(6)            | VARCHAR(6) COLLATE "C"            | P-AUTHFRDS-005
--   message_source         | MESSAGE_SOURCE         | CHAR(6)            | VARCHAR(6) COLLATE "C"            | P-AUTHFRDS-006
--   auth_id_code           | AUTH_ID_CODE           | CHAR(6)            | VARCHAR(6) COLLATE "C"            | P-AUTHFRDS-007
--   auth_resp_code         | AUTH_RESP_CODE         | CHAR(2)            | VARCHAR(2) COLLATE "C"            | P-AUTHFRDS-008
--   auth_resp_reason       | AUTH_RESP_REASON       | CHAR(4)            | VARCHAR(4) COLLATE "C"            | P-AUTHFRDS-009
--   processing_code        | PROCESSING_CODE        | CHAR(6)            | VARCHAR(6) COLLATE "C"            | P-AUTHFRDS-010
--   transaction_amt        | TRANSACTION_AMT        | DECIMAL(12,2)      | NUMERIC(12,2)                     | P-AUTHFRDS-011
--   approved_amt           | APPROVED_AMT           | DECIMAL(12,2)      | NUMERIC(12,2)                     | P-AUTHFRDS-012
--   merchant_catagory_code | MERCHANT_CATAGORY_CODE | CHAR(4)            | VARCHAR(4) COLLATE "C"            | P-AUTHFRDS-013
--   acqr_country_code      | ACQR_COUNTRY_CODE      | CHAR(3)            | VARCHAR(3) COLLATE "C"            | P-AUTHFRDS-014
--   pos_entry_mode         | POS_ENTRY_MODE         | SMALLINT           | SMALLINT                          | P-AUTHFRDS-015
--   merchant_id            | MERCHANT_ID            | CHAR(15)           | VARCHAR(15) COLLATE "C"           | P-AUTHFRDS-016
--   merchant_name          | MERCHANT_NAME          | VARCHAR(22)        | VARCHAR(22) COLLATE "C"           | P-AUTHFRDS-017
--   merchant_city          | MERCHANT_CITY          | CHAR(13)           | VARCHAR(13) COLLATE "C"           | P-AUTHFRDS-018
--   merchant_state         | MERCHANT_STATE         | CHAR(02)           | VARCHAR(2) COLLATE "C"            | P-AUTHFRDS-019
--   merchant_zip           | MERCHANT_ZIP           | CHAR(09)           | VARCHAR(9) COLLATE "C"            | P-AUTHFRDS-020
--   transaction_id         | TRANSACTION_ID         | CHAR(15)           | VARCHAR(15) COLLATE "C"           | P-AUTHFRDS-021
--   match_status           | MATCH_STATUS           | CHAR(1)            | VARCHAR(1) COLLATE "C"            | P-AUTHFRDS-022
--   auth_fraud             | AUTH_FRAUD             | CHAR(1)            | VARCHAR(1) COLLATE "C"            | P-AUTHFRDS-023
--   fraud_rpt_date         | FRAUD_RPT_DATE         | DATE               | DATE                              | P-AUTHFRDS-024
--   acct_id                | ACCT_ID                | DECIMAL(11)        | BIGINT, 0 to 99999999999          | P-AUTHFRDS-025
--   cust_id                | CUST_ID                | DECIMAL(9)         | BIGINT, 0 to 999999999            | P-AUTHFRDS-026
--   transaction_id is the Db2 TRANSACTION_ID CHAR(15), the authorization's own transaction
--   reference copied from PA-TRANSACTION-ID. It is unrelated to transactions.transaction_id,
--   the row identity of the transaction master in 0003, and no foreign key links the two.

-- The primary key replaces the DBPAUTP0 root sequence field ACCNTID and the DBPAUTX0
-- primary index on it [app/app-authorization-ims-db2-mq/ims/DBPAUTX0.dbd:L27-31], which
-- COPAUA0C, COPAUS0C and COPAUS1C read by GU WHERE ACCNTID and CBPAUP0C browses by GN in
-- key order. No foreign key links it to accounts: IMS enforces no relationship with the
-- VSAM files.
CREATE TABLE pending_auth_summaries (
    pa_acct_id           BIGINT                     NOT NULL PRIMARY KEY
        CHECK (pa_acct_id >= 0 AND pa_acct_id < 100000000000),
    pa_cust_id           INTEGER                    NOT NULL
        CHECK (pa_cust_id >= 0 AND pa_cust_id < 1000000000),
    pa_auth_status       VARCHAR(1)    COLLATE "C"  NOT NULL,
    pa_account_status    VARCHAR(2)[5] COLLATE "C"  NOT NULL
        CHECK (
            array_ndims(pa_account_status) = 1
            AND array_lower(pa_account_status, 1) = 1
            AND cardinality(pa_account_status) = 5
            AND array_position(pa_account_status, NULL) IS NULL
        ),
    pa_credit_limit      NUMERIC(11,2)              NOT NULL,
    pa_cash_limit        NUMERIC(11,2)              NOT NULL,
    pa_credit_balance    NUMERIC(11,2)              NOT NULL,
    pa_cash_balance      NUMERIC(11,2)              NOT NULL,
    pa_approved_auth_cnt SMALLINT                   NOT NULL
        CHECK (pa_approved_auth_cnt > -10000 AND pa_approved_auth_cnt < 10000),
    pa_declined_auth_cnt SMALLINT                   NOT NULL
        CHECK (pa_declined_auth_cnt > -10000 AND pa_declined_auth_cnt < 10000),
    pa_approved_auth_amt NUMERIC(11,2)              NOT NULL,
    pa_declined_auth_amt NUMERIC(11,2)              NOT NULL
);

-- A detail is a PAUTDTL1 twin under its summary.
--   pautdtl1_key is the unique sequence field PAUT9CTS within the parent, qualified by the
--   parent's key as the IMS concatenated key is. A second detail with the same key fails
--   with SQLSTATE 23505, the IMS II status of a duplicate ISRT, which COPAUA0C logs at I004.
--   COPAUA0C stores 99999 - YYDDD and 999999999 - HHMMSSmmm
--   [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L874-875]; for these non-negative
--   values integer order equals packed byte order, so ascending key order under an account
--   is newest first, the COPAUS0C and COPAUS1C GNP browse order (P-COPAUS0C-010).
--   The foreign key cascades because a child segment cannot exist without its root, and an
--   IMS DLET of the root deletes its dependents (P-CBPAUP0C-006). A detail inserted for a
--   missing summary fails with SQLSTATE 23503, the IMS GE status of an ISRT whose parent
--   is not found.
CREATE TABLE pending_auth_details (
    auth_id                   BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    pa_acct_id                BIGINT                   NOT NULL
        CHECK (pa_acct_id >= 0 AND pa_acct_id < 100000000000),
    pa_auth_date_9c           INTEGER                  NOT NULL
        CHECK (pa_auth_date_9c > -100000 AND pa_auth_date_9c < 100000),
    pa_auth_time_9c           INTEGER                  NOT NULL
        CHECK (pa_auth_time_9c > -1000000000 AND pa_auth_time_9c < 1000000000),
    pa_auth_orig_date         VARCHAR(6)  COLLATE "C"  NOT NULL,
    pa_auth_orig_time         VARCHAR(6)  COLLATE "C"  NOT NULL,
    pa_card_num               VARCHAR(16) COLLATE "C"  NOT NULL,
    pa_auth_type              VARCHAR(4)  COLLATE "C"  NOT NULL,
    pa_card_expiry_date       VARCHAR(4)  COLLATE "C"  NOT NULL,
    pa_message_type           VARCHAR(6)  COLLATE "C"  NOT NULL,
    pa_message_source         VARCHAR(6)  COLLATE "C"  NOT NULL,
    pa_auth_id_code           VARCHAR(6)  COLLATE "C"  NOT NULL,
    pa_auth_resp_code         VARCHAR(2)  COLLATE "C"  NOT NULL,
    pa_auth_resp_reason       VARCHAR(4)  COLLATE "C"  NOT NULL,
    pa_processing_code        INTEGER                  NOT NULL
        CHECK (pa_processing_code >= 0 AND pa_processing_code < 1000000),
    pa_transaction_amt        NUMERIC(12,2)            NOT NULL,
    pa_approved_amt           NUMERIC(12,2)            NOT NULL,
    pa_merchant_catagory_code VARCHAR(4)  COLLATE "C"  NOT NULL,
    pa_acqr_country_code      VARCHAR(3)  COLLATE "C"  NOT NULL,
    pa_pos_entry_mode         INTEGER                  NOT NULL
        CHECK (pa_pos_entry_mode >= 0 AND pa_pos_entry_mode < 100),
    pa_merchant_id            VARCHAR(15) COLLATE "C"  NOT NULL,
    pa_merchant_name          VARCHAR(22) COLLATE "C"  NOT NULL,
    pa_merchant_city          VARCHAR(13) COLLATE "C"  NOT NULL,
    pa_merchant_state         VARCHAR(2)  COLLATE "C"  NOT NULL,
    pa_merchant_zip           VARCHAR(9)  COLLATE "C"  NOT NULL,
    pa_transaction_id         VARCHAR(15) COLLATE "C"  NOT NULL,
    pa_match_status           VARCHAR(1)  COLLATE "C"  NOT NULL,
    pa_auth_fraud             VARCHAR(1)  COLLATE "C"  NOT NULL,
    pa_fraud_rpt_date         VARCHAR(8)  COLLATE "C"  NOT NULL,
    CONSTRAINT pautdtl1_key UNIQUE (pa_acct_id, pa_auth_date_9c, pa_auth_time_9c),
    CONSTRAINT pending_auth_details_pa_acct_id_fkey
        FOREIGN KEY (pa_acct_id) REFERENCES pending_auth_summaries (pa_acct_id)
        ON DELETE CASCADE
);

-- Columns in DDL order with the DDL nullability. The primary key (CARD_NUM, AUTH_TS)
-- [app/app-authorization-ims-db2-mq/ddl/AUTHFRDS.ddl:L28] makes a second fraud report of
-- the same authorization fail with SQLSTATE 23505, which persistence maps to the Db2
-- SQLCODE -803 path of COPAUS2C: update AUTH_FRAUD and FRAUD_RPT_DATE of the existing row
-- [app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl:L203-204]. AUTH_TS is the timestamp
-- COPAUS2C builds with TIMESTAMP_FORMAT 'YY-MM-DD HH24.MI.SSNNNNNN' from PA-AUTH-ORIG-DATE
-- and 999999999 - PA-AUTH-TIME-9C (P-COPAUS2C-001). No foreign key links the table to the
-- pending authorizations or the master files: Db2 declares none, and a purged detail
-- leaves its fraud row in place.
CREATE TABLE auth_fraud (
    card_num               VARCHAR(16) COLLATE "C" NOT NULL,
    auth_ts                TIMESTAMP(6)            NOT NULL,
    auth_type              VARCHAR(4)  COLLATE "C",
    card_expiry_date       VARCHAR(4)  COLLATE "C",
    message_type           VARCHAR(6)  COLLATE "C",
    message_source         VARCHAR(6)  COLLATE "C",
    auth_id_code           VARCHAR(6)  COLLATE "C",
    auth_resp_code         VARCHAR(2)  COLLATE "C",
    auth_resp_reason       VARCHAR(4)  COLLATE "C",
    processing_code        VARCHAR(6)  COLLATE "C",
    transaction_amt        NUMERIC(12,2),
    approved_amt           NUMERIC(12,2),
    merchant_catagory_code VARCHAR(4)  COLLATE "C",
    acqr_country_code      VARCHAR(3)  COLLATE "C",
    pos_entry_mode         SMALLINT,
    merchant_id            VARCHAR(15) COLLATE "C",
    merchant_name          VARCHAR(22) COLLATE "C",
    merchant_city          VARCHAR(13) COLLATE "C",
    merchant_state         VARCHAR(2)  COLLATE "C",
    merchant_zip           VARCHAR(9)  COLLATE "C",
    transaction_id         VARCHAR(15) COLLATE "C",
    match_status           VARCHAR(1)  COLLATE "C",
    auth_fraud             VARCHAR(1)  COLLATE "C",
    fraud_rpt_date         DATE,
    acct_id                BIGINT
        CHECK (acct_id >= 0 AND acct_id < 100000000000),
    cust_id                BIGINT
        CHECK (cust_id >= 0 AND cust_id < 1000000000),
    PRIMARY KEY (card_num, auth_ts)
);

-- The Db2 unique index XAUTHFRD (CARD_NUM ASC, AUTH_TS DESC)
-- [app/app-authorization-ims-db2-mq/ddl/XAUTHFRD.ddl:L1-4]: each card's fraud reports,
-- newest first.
CREATE UNIQUE INDEX xauthfrd ON auth_fraud (card_num ASC, auth_ts DESC);
