-- Pending authorizations and the fraud table. Replaces the IMS HIDAM database DBPAUTP0 (root
-- segment PAUTSUM0, child segment PAUTDTL1) and the Db2 table CARDDEMO.AUTHFRDS.
--
-- Sources:
--   [app/app-authorization-ims-db2-mq/cpy/CIPAUSMY.cpy:19-31]        PAUTSUM0 layout, 100 bytes
--   [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:19-54]        PAUTDTL1 layout, 200 bytes
--   [app/app-authorization-ims-db2-mq/ims/DBPAUTP0.dbd:28-37]        root key ACCNTID (SEQ,U),
--                                                                    child key PAUT9CTS (SEQ,U)
--   [app/app-authorization-ims-db2-mq/ddl/AUTHFRDS.ddl:1-28]         CARDDEMO.AUTHFRDS
--   [app/app-authorization-ims-db2-mq/dcl/AUTHFRDS.dcl:49-50,85-86]  ACCT_ID and CUST_ID
--   [app/app-authorization-ims-db2-mq/ddl/XAUTHFRD.ddl:1-4]          unique index XAUTHFRD
--   [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:798-931]      CP00's summary, detail writes
-- The field-level rows are CIPAUSMY-001 to -013 and CIPAUDTY-001 to -029 of
-- modern/audit/copybooks.md.
--
-- Column rules for the two IMS segments: X(n) becomes CHAR(n); 9(n) becomes BIGINT checked to
-- 0 .. 10^n - 1; an integral S9(n), packed or not, becomes BIGINT checked to
-- -(10^n - 1) .. 10^n - 1; S9(n)V99, packed or not, becomes NUMERIC(n+2,2); S9(4) COMP becomes
-- SMALLINT; FILLER becomes filler CHAR(n). Every segment column is NOT NULL, because a segment
-- always holds every field. Integral fields are BIGINT, so every account key is 64-bit end to end
-- (Checked Integer Narrowing). Amounts are NUMERIC only. The X(04), X(06) and X(08) date-like
-- fields stay text: Typed Dates and Timestamps covers the X(10) dates and X(26) timestamps, and
-- none is here. The Db2 table keeps its declared types, its DATE and TIMESTAMP columns included.
--
-- Every character column is COLLATE "C": IMS orders segment keys and COBOL compares alphanumeric
-- fields by their bytes, and the server's default locale must not change that order (the same
-- convention as migration 0001).
--
-- The only foreign key joins a detail to its summary, as the IMS hierarchy does. Neither IMS nor
-- Db2 declares a reference to the VSAM masters, so none joins these tables to accounts or
-- card_xref. sqlx applies this file in one transaction and records it as applied, so it runs once
-- per database. It only creates. This directory is the only schema source; a later change is a new
-- numbered file, never an edit of this one.

-- PAUTSUM0, the root segment [app/app-authorization-ims-db2-mq/cpy/CIPAUSMY.cpy:19-31]. The
-- primary key is the root key ACCNTID [app/app-authorization-ims-db2-mq/ims/DBPAUTP0.dbd:30].
-- That key is also the conflict target of CP00's INSERT ... ON CONFLICT (acct_id) DO NOTHING, and
-- the SELECT ... FOR UPDATE that follows locks the row: the lock serialises every authorization
-- decision for an account across worker processes and redeliveries (credit-limit invariant i10,
-- AAP 0.3.4.2). approved_auth_amt is the authoritative approved sum the decision reads under that
-- lock; the detail rows are never summed for it. The insert supplies zero amounts and counts, as
-- COPAUA0C's INITIALIZE ... REPLACING NUMERIC DATA BY ZERO does
-- [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:802-806].
CREATE TABLE pending_auth_summaries (
    -- PA-ACCT-ID PIC S9(11) COMP-3: signed, so the range is symmetric.
    acct_id           BIGINT        NOT NULL CHECK (acct_id BETWEEN -99999999999 AND 99999999999),
    -- PA-CUST-ID PIC 9(09).
    cust_id           BIGINT        NOT NULL CHECK (cust_id BETWEEN 0 AND 999999999),
    auth_status       CHAR(1)       COLLATE "C" NOT NULL,
    -- PA-ACCOUNT-STATUS PIC X(02) OCCURS 5 TIMES, one column per occurrence.
    account_status_1  CHAR(2)       COLLATE "C" NOT NULL,
    account_status_2  CHAR(2)       COLLATE "C" NOT NULL,
    account_status_3  CHAR(2)       COLLATE "C" NOT NULL,
    account_status_4  CHAR(2)       COLLATE "C" NOT NULL,
    account_status_5  CHAR(2)       COLLATE "C" NOT NULL,
    -- S9(09)V99 COMP-3.
    credit_limit      NUMERIC(11,2) NOT NULL,
    cash_limit        NUMERIC(11,2) NOT NULL,
    credit_balance    NUMERIC(11,2) NOT NULL,
    cash_balance      NUMERIC(11,2) NOT NULL,
    -- S9(04) COMP: two-byte binary, the range of SMALLINT.
    approved_auth_cnt SMALLINT      NOT NULL,
    declined_auth_cnt SMALLINT      NOT NULL,
    -- S9(09)V99 COMP-3.
    approved_auth_amt NUMERIC(11,2) NOT NULL,
    declined_auth_amt NUMERIC(11,2) NOT NULL,
    filler            CHAR(34)      COLLATE "C" NOT NULL,
    PRIMARY KEY (acct_id)
);

-- PAUTDTL1, the child segment [app/app-authorization-ims-db2-mq/cpy/CIPAUDTY.cpy:19-54]. IMS
-- keeps no account field in the segment: acct_id is the parent's ACCNTID, under which CP00 inserts
-- the detail [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:911-919]. The primary key is that
-- parent key followed by the unique sequence field PAUT9CTS, the two inverted keys
-- [app/app-authorization-ims-db2-mq/ims/DBPAUTP0.dbd:36-37]. Ascending key order is the IMS twin
-- order, newest first, and serves the COPAUS0C browse (endpoint 21, Bounded Pagination). CP00
-- builds the key from the clock to the millisecond
-- [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:857-875], so two authorizations for one
-- account in the same millisecond share a key: the second insert fails with SQLSTATE 23505, which
-- CP00 maps to its I004 path [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:925-931].
CREATE TABLE pending_auth_details (
    -- Parent key ACCNTID, PA-ACCT-ID PIC S9(11) COMP-3 of the summary.
    acct_id                BIGINT        NOT NULL
        CHECK (acct_id BETWEEN -99999999999 AND 99999999999),
    -- PA-AUTH-DATE-9C PIC S9(05) COMP-3: 99999 minus the YYDDD date.
    auth_date_9c           BIGINT        NOT NULL CHECK (auth_date_9c BETWEEN -99999 AND 99999),
    -- PA-AUTH-TIME-9C PIC S9(09) COMP-3: 999999999 minus (HHMMSS * 1000 + milliseconds).
    auth_time_9c           BIGINT        NOT NULL
        CHECK (auth_time_9c BETWEEN -999999999 AND 999999999),
    -- X(06) request date and time, kept as received.
    auth_orig_date         CHAR(6)       COLLATE "C" NOT NULL,
    auth_orig_time         CHAR(6)       COLLATE "C" NOT NULL,
    -- PA-CARD-NUM X(16): stored in full; only egress is redacted (Cardholder Data Redaction).
    card_num               CHAR(16)      COLLATE "C" NOT NULL,
    auth_type              CHAR(4)       COLLATE "C" NOT NULL,
    -- X(04) MMYY text, not a typed date.
    card_expiry_date       CHAR(4)       COLLATE "C" NOT NULL,
    message_type           CHAR(6)       COLLATE "C" NOT NULL,
    message_source         CHAR(6)       COLLATE "C" NOT NULL,
    auth_id_code           CHAR(6)       COLLATE "C" NOT NULL,
    auth_resp_code         CHAR(2)       COLLATE "C" NOT NULL,
    auth_resp_reason       CHAR(4)       COLLATE "C" NOT NULL,
    -- PA-PROCESSING-CODE PIC 9(06).
    processing_code        BIGINT        NOT NULL CHECK (processing_code BETWEEN 0 AND 999999),
    -- S9(10)V99 COMP-3.
    transaction_amt        NUMERIC(12,2) NOT NULL,
    approved_amt           NUMERIC(12,2) NOT NULL,
    -- The copybook's spelling is kept.
    merchant_catagory_code CHAR(4)       COLLATE "C" NOT NULL,
    acqr_country_code      CHAR(3)       COLLATE "C" NOT NULL,
    -- PA-POS-ENTRY-MODE PIC 9(02).
    pos_entry_mode         BIGINT        NOT NULL CHECK (pos_entry_mode BETWEEN 0 AND 99),
    merchant_id            CHAR(15)      COLLATE "C" NOT NULL,
    merchant_name          CHAR(22)      COLLATE "C" NOT NULL,
    merchant_city          CHAR(13)      COLLATE "C" NOT NULL,
    merchant_state         CHAR(2)       COLLATE "C" NOT NULL,
    merchant_zip           CHAR(9)       COLLATE "C" NOT NULL,
    transaction_id         CHAR(15)      COLLATE "C" NOT NULL,
    match_status           CHAR(1)       COLLATE "C" NOT NULL,
    auth_fraud             CHAR(1)       COLLATE "C" NOT NULL,
    -- X(08) text, not a typed date.
    fraud_rpt_date         CHAR(8)       COLLATE "C" NOT NULL,
    filler                 CHAR(17)      COLLATE "C" NOT NULL,
    PRIMARY KEY (acct_id, auth_date_9c, auth_time_9c),
    -- IMS deletes child segments with their parent. CBPAUP0C deletes a summary whose approved
    -- count as read, less one for each expired approval it deletes in that run, is 0 or less; the
    -- test names the approved count twice and the declined count never (CBPAUP0C-016,
    -- X-CBPAUP0J-2), and every detail still under the summary, declined or approved and however
    -- recent, goes with it
    -- [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:156-158,284-293,328-338]. The
    -- primary key's leading acct_id serves this key, so no further index exists.
    CONSTRAINT pending_auth_details_summary_fk FOREIGN KEY (acct_id)
        REFERENCES pending_auth_summaries (acct_id) ON DELETE CASCADE
);

-- CARDDEMO.AUTHFRDS [app/app-authorization-ims-db2-mq/ddl/AUTHFRDS.ddl:1-28], written by COPAUS2C.
-- Columns, order and nullability are the DDL's. Db2 types are kept as declared: DECIMAL(12,2) is
-- NUMERIC(12,2), and TIMESTAMP carries 6 fractional digits in Db2, hence TIMESTAMP(6).
CREATE TABLE auth_fraud (
    card_num               CHAR(16)      COLLATE "C" NOT NULL,
    auth_ts                TIMESTAMP(6)  NOT NULL,
    auth_type              CHAR(4)       COLLATE "C",
    card_expiry_date       CHAR(4)       COLLATE "C",
    message_type           CHAR(6)       COLLATE "C",
    message_source         CHAR(6)       COLLATE "C",
    auth_id_code           CHAR(6)       COLLATE "C",
    auth_resp_code         CHAR(2)       COLLATE "C",
    auth_resp_reason       CHAR(4)       COLLATE "C",
    processing_code        CHAR(6)       COLLATE "C",
    transaction_amt        NUMERIC(12,2),
    approved_amt           NUMERIC(12,2),
    merchant_catagory_code CHAR(4)       COLLATE "C",
    acqr_country_code      CHAR(3)       COLLATE "C",
    pos_entry_mode         SMALLINT,
    merchant_id            CHAR(15)      COLLATE "C",
    merchant_name          VARCHAR(22)   COLLATE "C",
    merchant_city          CHAR(13)      COLLATE "C",
    merchant_state         CHAR(2)       COLLATE "C",
    merchant_zip           CHAR(9)       COLLATE "C",
    transaction_id         CHAR(15)      COLLATE "C",
    match_status           CHAR(1)       COLLATE "C",
    auth_fraud             CHAR(1)       COLLATE "C",
    fraud_rpt_date         DATE,
    -- ACCT_ID DECIMAL(11) and CUST_ID DECIMAL(9) have no fraction digits (DCLGEN PIC S9(11)V and
    -- S9(9)V COMP-3, [app/app-authorization-ims-db2-mq/dcl/AUTHFRDS.dcl:49-50,85-86]): they hold
    -- integral identifiers. Checked Integer Narrowing and the BIGINT account key therefore govern
    -- them in place of the declared DECIMAL, with the declared digit count as the range;
    -- domain::online::copaus2c::AuthFraudRow carries them as PicU<11> and PicU<9>.
    acct_id                BIGINT        CHECK (acct_id BETWEEN -99999999999 AND 99999999999),
    cust_id                BIGINT        CHECK (cust_id BETWEEN -999999999 AND 999999999),
    PRIMARY KEY (card_num, auth_ts)
);

-- [app/app-authorization-ims-db2-mq/ddl/XAUTHFRD.ddl:1-4]
CREATE UNIQUE INDEX xauthfrd ON auth_fraud (card_num ASC, auth_ts DESC);
