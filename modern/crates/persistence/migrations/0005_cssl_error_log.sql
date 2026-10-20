-- CP00 error log, the replacement for the CICS transient data queue CSSL. COPAUA0C's paragraph
-- 9500-LOG-ERROR writes ERROR-LOG-RECORD there with WRITEQ TD QUEUE('CSSL')
-- [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:983-1010]. Only messaging::workers::cp00
-- inserts rows, through persistence::repos::cssl_error_log (AAP 0.5.2).
--
-- Source: the 122-byte ERROR-LOG-RECORD [app/app-authorization-ims-db2-mq/cpy/CCPAUERY.cpy:19-40],
-- audited as rows CCPAUERY-001 to CCPAUERY-012 of modern/audit/copybooks.md. Its eleven fields
-- are eleven columns in copybook order, each X(n) as CHAR(n), which keeps and returns the space
-- padding:
--   err_date, err_time         FORMATTIME YYMMDD and TIME (HHMMSS) text [COPAUA0C.cbl:990-999],
--                              kept as text: Typed Dates and Timestamps covers only the X(10)
--                              dates and the X(26) timestamps
--   err_application            the CICS transaction ID, CP00 [COPAUA0C.cbl:34,996]
--   err_program                COPAUA0C [COPAUA0C.cbl:33,997]
--   err_location               the log location code, for example M004, I003, I004
--   err_level, err_subsystem   the 88-level codes of CCPAUERY.cpy:25-36; no CHECK lists them,
--                              because domain::records::error_log::{ErrorLevel, ErrorSubsystem}
--                              types them
--   err_code_1, err_code_2     CICS, MQ and IMS status codes (halt H4)
--
-- A TDQ record has no key, so the table carries the surrogate key id, which is not a copybook
-- field. BIGSERIAL is a 64-bit BIGINT, read as i64 (Checked Integer Narrowing); codec B neither
-- writes nor compares it. No foreign key and no secondary index: no join and no paginated query
-- reads this table.
--
-- A CSSL write is not part of the CICS unit of work. Each row is therefore inserted and committed
-- in its own transaction, independent of the CP00 authorization transaction that holds the i10
-- lock on the pending_auth_summaries row, and it stands whether or not that transaction commits
-- (AAP 0.5.2, 0.3.4.2).
--
-- Cardholder Data Redaction, escalation E32: COPAUA0C moves full card numbers into ERR-EVENT-KEY
-- [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:428,499,511]. err_event_key stores a card
-- number only in its 16-character redacted form, twelve * followed by the last four digits and
-- space-padded to 20; account-ID and customer-ID keys are stored as written. The schema cannot
-- enforce the redaction; the writer does, through ErrorLogRecord::card_event_key.
--
-- Every character column is COLLATE "C", as in migration 0001, so values compare as the bytes the
-- COBOL wrote, whatever the server's locale. sqlx applies this file in one transaction through
-- runtime::migrate, the only schema executor. This directory is the only schema source; a later
-- change is a new numbered file, never an edit of this one.

CREATE TABLE cssl_error_log (
    id              BIGSERIAL            NOT NULL,
    err_date        CHAR(6)  COLLATE "C" NOT NULL,
    err_time        CHAR(6)  COLLATE "C" NOT NULL,
    err_application CHAR(8)  COLLATE "C" NOT NULL,
    err_program     CHAR(8)  COLLATE "C" NOT NULL,
    err_location    CHAR(4)  COLLATE "C" NOT NULL,
    err_level       CHAR(1)  COLLATE "C" NOT NULL,
    err_subsystem   CHAR(1)  COLLATE "C" NOT NULL,
    err_code_1      CHAR(9)  COLLATE "C" NOT NULL,
    err_code_2      CHAR(9)  COLLATE "C" NOT NULL,
    err_message     CHAR(50) COLLATE "C" NOT NULL,
    err_event_key   CHAR(20) COLLATE "C" NOT NULL,
    CONSTRAINT cssl_error_log_pkey PRIMARY KEY (id)
);
