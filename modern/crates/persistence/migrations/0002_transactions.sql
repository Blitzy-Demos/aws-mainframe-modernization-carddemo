-- The transaction master: TRAN-RECORD of CVTRA05Y [app/cpy/CVTRA05Y.cpy:4-18], 350 bytes.
--
-- This file performs TRANFILE's DEFINE CLUSTER, a KSDS with KEYS(16 0) [app/jcl/TRANFILE.jcl:53],
-- and TRANIDX's alternate index on the processing timestamp, KEYS(26 304)
-- [app/jcl/TRANIDX.jcl:25-27]. The table starts empty, which is TRANFILE's initialised file: its
-- REPRO input exists only in EBCDIC. Rows come from posttran (CBTRN02C), transaction add (COTRN02C)
-- and bill payment (COBIL00C).
--
-- DALYTRAN-RECORD of CVTRA06Y [app/cpy/CVTRA06Y.cpy:4-18] has the same fields, offsets and PICs
-- under the DALYTRAN- prefix, so the codec B round trip (d9) of app/data/ASCII/dailytran.txt runs
-- through this row shape.
--
-- Column rules (modern/audit/copybooks.md, rows CVTRA05Y-001 to CVTRA05Y-014):
--   X(n)       -> CHAR(n) COLLATE "C". PostgreSQL keeps and returns the padding, and "C" compares
--                 and orders by bytes, as VSAM keys and COBOL alphanumeric compares do, whatever
--                 the server's default locale.
--   9(n)       -> BIGINT with a literal 0 to 10^n - 1 range check (Checked Integer Narrowing).
--   S9(09)V99  -> NUMERIC(11,2), an exact decimal that holds the PIC's whole range (R1).
--   X(26) time -> TIMESTAMP(6) plus a CHAR(1) layout code <col>_form (Typed Dates and Timestamps);
--                 the six fraction digits of every layout fit the microsecond precision.
--   FILLER     -> filler CHAR(20), kept byte for byte.
-- No foreign key: VSAM declares none, so posttran and the fixture loads stay order-independent.
--
-- E33 (modern/audit/ESCALATIONS.md): TRAN-ORIG-TS and TRAN-PROC-TS are written in several text
-- layouts, or left blank. The value is stored typed, and the layout code records which text the
-- COBOL wrote, so codec B re-encodes the original 26 bytes. The writers:
--   [app/cbl/CBTRN02C.cbl:436-438]  TRAN-ORIG-TS copied from DALYTRAN-ORIG-TS (I);
--                                   TRAN-PROC-TS from DB2-FORMAT-TS (D)
--   [app/cbl/CBACT04C.cbl:497]      DB2-FORMAT-TS (D) in both fields of its SYSTRAN records
--   [app/cbl/COTRN02C.cbl:464-465]  the screen's original and processing dates (S)
--   [app/cbl/COBIL00C.cbl:231-232]  its assembled current timestamp (I) in both fields
--
-- sqlx applies this file in one transaction, and only runtime::migrate applies it. This directory
-- is the only schema source; a later change is a new numbered file, never an edit of this one.

CREATE TABLE transactions (
    tran_id            CHAR(16)      COLLATE "C" NOT NULL,
    tran_type_cd       CHAR(2)       COLLATE "C" NOT NULL,
    tran_cat_cd        BIGINT        NOT NULL CHECK (tran_cat_cd BETWEEN 0 AND 9999),
    tran_source        CHAR(10)      COLLATE "C" NOT NULL,
    tran_desc          CHAR(100)     COLLATE "C" NOT NULL,
    tran_amt           NUMERIC(11,2) NOT NULL,
    tran_merchant_id   BIGINT        NOT NULL CHECK (tran_merchant_id BETWEEN 0 AND 999999999),
    tran_merchant_name CHAR(50)      COLLATE "C" NOT NULL,
    tran_merchant_city CHAR(50)      COLLATE "C" NOT NULL,
    tran_merchant_zip  CHAR(10)      COLLATE "C" NOT NULL,
    card_num           CHAR(16)      COLLATE "C" NOT NULL,
    -- Layout codes: I = 'YYYY-MM-DD HH:MM:SS.ffffff' (all 300 DALYTRAN-ORIG-TS values);
    -- D = the Db2 form 'YYYY-MM-DD-HH.MM.SS.ffffff'; S = a 10-character screen date followed by
    -- 16 spaces, stored at midnight (PIC-020). A NULL timestamp with a NULL form is 26 spaces
    -- (all 300 DALYTRAN-PROC-TS values). The checks below admit only these audited codes, which
    -- copybook::TsForm decodes (PIC-018 to PIC-021). A further layout an audited writer produces
    -- gets its code in modern/audit/copybooks.md first, then a later numbered migration admits it.
    orig_ts            TIMESTAMP(6),
    orig_ts_form       CHAR(1)       COLLATE "C",
    proc_ts            TIMESTAMP(6),
    proc_ts_form       CHAR(1)       COLLATE "C",
    filler             CHAR(20)      COLLATE "C" NOT NULL,
    -- KSDS KEYS(16 0). COTRN00C's browse (endpoint 11) pages on this index.
    CONSTRAINT transactions_pkey PRIMARY KEY (tran_id),
    -- A timestamp and its layout code are present together or absent together.
    CONSTRAINT transactions_orig_ts_form_ck CHECK ((orig_ts IS NULL) = (orig_ts_form IS NULL)),
    CONSTRAINT transactions_proc_ts_form_ck CHECK ((proc_ts IS NULL) = (proc_ts_form IS NULL)),
    -- A layout code is one of the audited I, D and S; a NULL code is left to the checks above.
    CONSTRAINT transactions_orig_ts_form_code_ck CHECK (orig_ts_form IN ('I', 'D', 'S')),
    CONSTRAINT transactions_proc_ts_form_code_ck CHECK (proc_ts_form IN ('I', 'D', 'S')),
    -- A form-S value is its screen date at 00:00:00.000000, the only time it encodes (PIC-020).
    CONSTRAINT transactions_orig_ts_screen_date_ck
        CHECK (orig_ts_form <> 'S' OR CAST(orig_ts AS TIME) = TIME '00:00:00'),
    CONSTRAINT transactions_proc_ts_screen_date_ck
        CHECK (proc_ts_form <> 'S' OR CAST(proc_ts AS TIME) = TIME '00:00:00')
);

-- Replaces the AIX KEYS(26 304), NONUNIQUEKEY [app/jcl/TRANIDX.jcl:25-28]: it serves tranrept's
-- window on the processing date [app/proc/TRANREPT.prc:45-46].
CREATE INDEX transactions_proc_ts_idx ON transactions (proc_ts);

-- Serves the card-ordered reads that CREASTMT obtains by SORT FIELDS=(263,16,CH,A,1,16,CH,A),
-- card number then transaction ID [app/jcl/CREASTMT.JCL:53], and TRANREPT by its card-number
-- SORT [app/proc/TRANREPT.prc:44]. It has no VSAM counterpart.
CREATE INDEX transactions_card_num_idx ON transactions (card_num, tran_id);
