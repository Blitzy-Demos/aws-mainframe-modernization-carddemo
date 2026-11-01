-- Transaction schema: the VSAM transaction master TRANSACT with its alternate index on the
-- processing timestamp, and the transaction category balance file TCATBALF. It replaces the
-- IDCAMS define steps of TRANFILE, TRANIDX and TCATBALF. It holds no rows: `batch seed`
-- loads the 50 tcatbal.txt records into transaction_category_balances (P-CVTRA01Y-006), and
-- transactions starts empty (P-CVTRA05Y-015, E-07 below).
--
-- Sources:
--   app/cpy/CVTRA05Y.cpy, CVTRA01Y.cpy             record layouts
--   app/jcl/TRANFILE.jcl, TRANIDX.jcl, TCATBALF.jcl KSDS keys, record sizes, alternate index
--   app/jcl/TRANBKP.jcl, COMBTRAN.jcl               emptying and reloading the transaction master
--   app/csd/CARDDEMO.CSD                            CICS file TRANSACT
--   docs/parity/parity-oracle-audit.md, section A   reviewed layout rows cited below
--
-- Conventions (shared with 0001 and 0002):
--   X(n) maps to VARCHAR(n) COLLATE "C". The database default collation (en_US.utf8 in
--   postgres:16.15) does not sort by byte value; "C" gives the byte order of the KSDS keys
--   (the COTRN00C browse by TRAN-ID) and of COBOL string comparisons.
--   Text is stored right-trimmed of trailing spaces; the codec and persistence layers pad
--   it back to the PIC width. The X(26) timestamps stay text in the form their writer
--   produced (dailytran.txt holds 2022-06-10 19:27:53.000000; CBTRN02C and CBACT04C stamp
--   the Db2 form 2022-07-01-12.00.00.000000 [app/cbl/CBTRN02C.cbl:L692-705]), so they
--   compare and sort as text, as the alternate index on TRAN-PROC-TS does.
--   Unsigned 9(n) maps to INTEGER for n <= 9 and BIGINT for n >= 10, with
--   CHECK (0 <= v < 10^n).
--   S9(09)V99 maps to NUMERIC(11,2): the precision bounds the value to the 9 integer digits
--   of the PIC and the scale to its 2 decimals.
--   Every copybook column is NOT NULL, as every field of a VSAM record holds a value.
--   Text columns carry no value-domain check.
--   FILLER is not stored; the codec layouts keep its bytes.
--   No foreign keys link these tables to the master files of 0002. VSAM enforces no
--   relationship between the files: CBTRN02C writes the transaction and creates the
--   category balance of every record that passed its own card and account lookups
--   [app/cbl/CBTRN02C.cbl:L424-442], and CT02 and CB00 write the transaction master after
--   their own lookups, with no cross-file constraint.
--   No column defaults to the current time: writers supply the processing clock
--   (CARDDEMO_CLOCK_OVERRIDE when set).
--
-- E-07. TRANFILE loads the init record DALYTRAN.PS.INIT into TRANSACT
-- [app/jcl/TRANFILE.jcl:L67-74]. Its 350 bytes are 342 X'00' bytes, then EBCDIC 00000100 at
-- offsets 342-349 (P-CVTRA05Y-019; E-43 corrects the 340 X'00' bytes and 0000000100 first
-- recorded). PostgreSQL text cannot hold X'00', so the record is not loaded and
-- transactions starts empty (P-CVTRA05Y-017): the state the README batch sequence
-- produces, as it runs TRANBKP before POSTTRAN and runs no TRANFILE [README.md:L213-232].
--
-- E-09, the live-master representation. TRANBKP deletes and redefines the TRANSACT cluster
-- [app/jcl/TRANBKP.jcl:L35-67], but migrations/0005_rewards.sql makes
-- reward_accruals.transaction_id reference transactions.transaction_id (ON DELETE
-- RESTRICT), so a transaction an accrual points at cannot be deleted. The table therefore
-- keeps such rows, and the live TRANSACT file is the subset of rows with in_master:
--   transaction_id  the row identity and the foreign key target, generated, never reused.
--   tran_id         the COBOL TRAN-ID, the KSDS key. It is unique among live rows only: a
--                   TRAN-ID held only by non-live rows may be issued again, as the COBOL
--                   issues it again on an emptied file.
--   in_master       true for the rows of the live TRANSACT file.
-- `batch tranbkp` writes TRANSACT.BKUP(+1) from the live rows, then sets in_master = false
-- on every live row and deletes the non-live rows no accrual references (P-TRANBKP-004,
-- P-TRANBKP-005). `batch combtran` loads TRANSACT.COMBINED(+1) as the REPRO without
-- REPLACE it replaces [app/jcl/COMBTRAN.jcl:L41-48]: a record whose tran_id is already live
-- is not loaded (P-COMBTRAN-003), a non-live row equal in tran_id and content is made live
-- again, keeping its accruals linked, and any other record is inserted as a new live row.
--
-- Read path. Every legacy read of TRANSACT (CT00, CT01, the CT02 PF5 copy, POSTTRAN,
-- CREASTMT, TRANREPT, CR00) and the TRAN-ID allocation in crates/persistence/src/ids.rs go
-- through the view transaction_master. The allocation takes pg_advisory_xact_lock, then
-- max(tran_id) + 1 zero-padded to 16: the highest key + 1 that COTRN02C and COBIL00C read
-- by READPREV from HIGH-VALUES [app/cbl/COTRN02C.cbl:L442-451]
-- [app/cbl/COBIL00C.cbl:L212-219]. Only crates/persistence/src/repos/transactions.rs and
-- crates/persistence/src/rewards.rs name the base table in runtime SQL
-- (crates/testkit/tests/hygiene.rs enforces it).
--
-- sqlx: PostgreSQL marks every view column nullable (pg_attribute.attnotnull is false for
-- a view), so query! and query_as! over transaction_master infer Option for every column.
-- Persistence code declares each column non-null with an `as "col!"` override.
--
-- transactions: VSAM TRANSACT, CVTRA05Y TRAN-RECORD, RECLN 350, KEYS(16 0) [app/jcl/TRANFILE.jcl:L49-54]
--   column             | copybook field     | PIC       | SQL type                               | audit row
--   transaction_id     | (none)             | (none)    | BIGINT NOT NULL, identity, primary key | new (E-09)
--   tran_id            | TRAN-ID            | X(16)     | VARCHAR(16) COLLATE "C" NOT NULL       | P-CVTRA05Y-001
--   tran_type_cd       | TRAN-TYPE-CD       | X(02)     | VARCHAR(2) COLLATE "C" NOT NULL        | P-CVTRA05Y-002
--   tran_cat_cd        | TRAN-CAT-CD        | 9(04)     | INTEGER NOT NULL, 0 to 9999            | P-CVTRA05Y-003
--   tran_source        | TRAN-SOURCE        | X(10)     | VARCHAR(10) COLLATE "C" NOT NULL       | P-CVTRA05Y-004
--   tran_desc          | TRAN-DESC          | X(100)    | VARCHAR(100) COLLATE "C" NOT NULL      | P-CVTRA05Y-005
--   tran_amt           | TRAN-AMT           | S9(09)V99 | NUMERIC(11,2) NOT NULL                 | P-CVTRA05Y-006
--   tran_merchant_id   | TRAN-MERCHANT-ID   | 9(09)     | INTEGER NOT NULL, 0 to 999999999       | P-CVTRA05Y-007
--   tran_merchant_name | TRAN-MERCHANT-NAME | X(50)     | VARCHAR(50) COLLATE "C" NOT NULL       | P-CVTRA05Y-008
--   tran_merchant_city | TRAN-MERCHANT-CITY | X(50)     | VARCHAR(50) COLLATE "C" NOT NULL       | P-CVTRA05Y-009
--   tran_merchant_zip  | TRAN-MERCHANT-ZIP  | X(10)     | VARCHAR(10) COLLATE "C" NOT NULL       | P-CVTRA05Y-010
--   tran_card_num      | TRAN-CARD-NUM      | X(16)     | VARCHAR(16) COLLATE "C" NOT NULL       | P-CVTRA05Y-011
--   tran_orig_ts       | TRAN-ORIG-TS       | X(26)     | VARCHAR(26) COLLATE "C" NOT NULL       | P-CVTRA05Y-012
--   tran_proc_ts       | TRAN-PROC-TS       | X(26)     | VARCHAR(26) COLLATE "C" NOT NULL       | P-CVTRA05Y-013
--   (not stored)       | FILLER             | X(20)     | not stored                             | P-CVTRA05Y-014
--   in_master          | (none)             | (none)    | BOOLEAN NOT NULL DEFAULT TRUE          | new (E-09)
--   POSTTRAN and INTCALC never set the FILLER (E-25); the replacement writes it as spaces
--   (P-CVTRA05Y-018).
--
-- transaction_category_balances: VSAM TCATBALF, CVTRA01Y TRAN-CAT-BAL-RECORD, RECLN 50,
-- KEYS(17 0) [app/jcl/TCATBALF.jcl:L36-41]
--   column          | copybook field  | PIC       | SQL type                          | audit row
--   trancat_acct_id | TRANCAT-ACCT-ID | 9(11)     | BIGINT NOT NULL, 0 to 99999999999 | P-CVTRA01Y-001
--   trancat_type_cd | TRANCAT-TYPE-CD | X(02)     | VARCHAR(2) COLLATE "C" NOT NULL   | P-CVTRA01Y-002
--   trancat_cd      | TRANCAT-CD      | 9(04)     | INTEGER NOT NULL, 0 to 9999       | P-CVTRA01Y-003
--   tran_cat_bal    | TRAN-CAT-BAL    | S9(09)V99 | NUMERIC(11,2) NOT NULL            | P-CVTRA01Y-004
--   (not stored)    | FILLER          | X(22)     | not stored                        | P-CVTRA01Y-005
--   The key columns keep their copybook names, because the request names only entity
--   primary keys. The fixture FILLER holds '0' characters, kept only by the codec layout.

-- transaction_id is GENERATED ALWAYS, so no writer supplies it: COMBTRAN revives a non-live
-- row by updating it, never by inserting its old identity.
CREATE TABLE transactions (
    transaction_id     BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tran_id            VARCHAR(16)  COLLATE "C" NOT NULL,
    tran_type_cd       VARCHAR(2)   COLLATE "C" NOT NULL,
    tran_cat_cd        INTEGER                  NOT NULL
        CHECK (tran_cat_cd >= 0 AND tran_cat_cd < 10000),
    tran_source        VARCHAR(10)  COLLATE "C" NOT NULL,
    tran_desc          VARCHAR(100) COLLATE "C" NOT NULL,
    tran_amt           NUMERIC(11,2)            NOT NULL,
    tran_merchant_id   INTEGER                  NOT NULL
        CHECK (tran_merchant_id >= 0 AND tran_merchant_id < 1000000000),
    tran_merchant_name VARCHAR(50)  COLLATE "C" NOT NULL,
    tran_merchant_city VARCHAR(50)  COLLATE "C" NOT NULL,
    tran_merchant_zip  VARCHAR(10)  COLLATE "C" NOT NULL,
    tran_card_num      VARCHAR(16)  COLLATE "C" NOT NULL,
    tran_orig_ts       VARCHAR(26)  COLLATE "C" NOT NULL,
    tran_proc_ts       VARCHAR(26)  COLLATE "C" NOT NULL,
    in_master          BOOLEAN                  NOT NULL DEFAULT TRUE
);

-- The KSDS primary key KEYS(16 0) [app/jcl/TRANFILE.jcl:L53] over the live rows. It serves
-- the CT00 browse order, the max(tran_id) allocation and duplicate detection: a second
-- live row with the same tran_id fails with SQLSTATE 23505, the DUPKEY/DUPREC outcome of
-- the COTRN02C and COBIL00C writes [app/cbl/COTRN02C.cbl:L735-736]
-- [app/cbl/COBIL00C.cbl:L533-534] and the IDC3308I duplicate of the COMBTRAN REPRO.
CREATE UNIQUE INDEX transactions_master_tran_id ON transactions (tran_id) WHERE in_master;

-- Replaces the non-unique alternate index KEYS(26 304) on TRAN-PROC-TS, defined by
-- TRANFILE [app/jcl/TRANFILE.jcl:L82-86] and again by TRANIDX [app/jcl/TRANIDX.jcl:L25-28]
-- over the live file.
CREATE INDEX transactions_master_proc_ts ON transactions (tran_proc_ts) WHERE in_master;

-- The live TRANSACT file. A simple view over one table, so PostgreSQL makes it
-- automatically updatable: an INSERT through it takes in_master = TRUE from the column
-- default, and WITH LOCAL CHECK OPTION rejects any INSERT or UPDATE through it whose row
-- would not be live. in_master is not a view column; only repos/transactions.rs changes it,
-- on the base table, for TRANBKP and COMBTRAN.
CREATE VIEW transaction_master AS
SELECT
    transaction_id,
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
    tran_card_num,
    tran_orig_ts,
    tran_proc_ts
FROM transactions
WHERE in_master
WITH LOCAL CHECK OPTION;

-- The primary key is the KSDS key TRAN-CAT-KEY. CBTRN02C reads by it and creates the
-- record on status 23 (2700-UPDATE-TCATBAL, 2700-A, 2700-B) [app/cbl/CBTRN02C.cbl:L467-543];
-- CBACT04C browses the file sequentially in key order [app/cbl/CBACT04C.cbl:L28-32]. No
-- foreign key to accounts, as in VSAM.
CREATE TABLE transaction_category_balances (
    trancat_acct_id BIGINT                 NOT NULL
        CHECK (trancat_acct_id >= 0 AND trancat_acct_id < 100000000000),
    trancat_type_cd VARCHAR(2) COLLATE "C" NOT NULL,
    trancat_cd      INTEGER                NOT NULL
        CHECK (trancat_cd >= 0 AND trancat_cd < 10000),
    tran_cat_bal    NUMERIC(11,2)          NOT NULL,
    PRIMARY KEY (trancat_acct_id, trancat_type_cd, trancat_cd)
);
