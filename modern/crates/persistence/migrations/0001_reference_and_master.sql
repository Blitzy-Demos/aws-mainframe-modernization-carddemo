-- VSAM data version of the CardDemo master and reference files: accounts, customers, cards, the
-- card cross-reference, transaction types and categories, disclosure groups and transaction
-- category balances. Each table stands in for the KSDS that an IDCAMS DEFINE CLUSTER creates in
-- its load job, and each secondary index for a DEFINE ALTERNATEINDEX. The load subcommands
-- acctfile, custfile, cardfile, xreffile, trantype, trancatg, discgrp and tcatbalf fill them from
-- app/data/ASCII through codec A and codec B, as each job's REPRO fills its cluster.
--
-- Sources (record layout; KSDS key; alternate index):
--   accounts                [app/cpy/CVACT01Y.cpy:4-17]  KEYS(11 0) [app/jcl/ACCTFILE.jcl:40]
--   customers               [app/cpy/CVCUS01Y.cpy:4-23]  KEYS(9 0)  [app/jcl/CUSTFILE.jcl:50]
--   cards                   [app/cpy/CVACT02Y.cpy:4-11]  KEYS(16 0) [app/jcl/CARDFILE.jcl:54]
--                           AIX KEYS(11 16) [app/jcl/CARDFILE.jcl:83-86]
--   card_xref               [app/cpy/CVACT03Y.cpy:4-8]   KEYS(16 0) [app/jcl/XREFFILE.jcl:43]
--                           AIX KEYS(11,25) [app/jcl/XREFFILE.jcl:72-75]
--   transaction_types       [app/cpy/CVTRA03Y.cpy:4-7]   KEYS(2 0)  [app/jcl/TRANTYPE.jcl:40]
--   transaction_categories  [app/cpy/CVTRA04Y.cpy:4-9]   KEYS(6 0)  [app/jcl/TRANCATG.jcl:40]
--   disclosure_groups       [app/cpy/CVTRA02Y.cpy:4-10]  KEYS(16 0) [app/jcl/DISCGRP.jcl:40]
--   tran_cat_balances       [app/cpy/CVTRA01Y.cpy:4-10]  KEYS(17 0) [app/jcl/TCATBALF.jcl:40]
-- Column names and types are the rows CVACT01Y-001 to CVTRA04Y-004 of modern/audit/copybooks.md.
-- Columns follow copybook field order, and each record's widths add up to its RECORDSIZE.
--
-- Column rules, applied per copybook field (copybooks.md PIC-001 to PIC-016):
--   PIC X(n)         CHAR(n) NOT NULL. PostgreSQL keeps and returns the space padding, so codec B
--                    reads back the bytes it stored.
--   PIC X(10) date   DATE, nullable, NULL standing for 10 spaces (Typed Dates and Timestamps). Only
--                    acct_open_date, acct_expiraion_date, acct_reissue_date, card_expiraion_date
--                    and cust_dob_yyyy_mm_dd; the copybook spelling EXPIRAION is kept.
--   PIC 9(n)         BIGINT NOT NULL CHECK (col BETWEEN 0 AND 10^n - 1), the bound written as n
--                    nines, so every integral field is 64-bit (Checked Integer Narrowing) and a
--                    value the PIC cannot hold is refused.
--   PIC S9(n)V9(m)   NUMERIC(n+m, m) NOT NULL: exactly the PIC's range and scale, so money and the
--                    disclosure rate stay exact decimals (R1).
--   FILLER           filler CHAR(n) NOT NULL, kept byte for byte: the discgrp, tcatbal, trancatg
--                    and trantype fixtures carry non-space filler (E05).
--
-- Every character column is COLLATE "C". COBOL alphanumeric comparisons and KSDS key order are
-- binary byte comparisons, while the postgres:16 image's default locale (en_US.utf8) would order
-- mixed-case or space-bearing keys differently and make browse and paging order depend on the
-- server. The data type stays CHAR(n), whose comparison ignores trailing spaces as a COBOL
-- comparison pads the shorter operand with spaces. A 9(n) key part is zero-padded to its width in
-- the record, so its BIGINT order equals the byte order of its digits, and every composite
-- primary key orders as its KSDS key does.
--
-- No table carries a foreign key, because VSAM declares none: each load subcommand replaces one
-- table inside one transaction, so the loads succeed in any order, on an empty or a loaded
-- database (AAP 0.3.3.4). Primary keys take PostgreSQL's default name <table>_pkey.
--
-- sqlx applies this file in one transaction, hence no BEGIN or COMMIT here, through
-- runtime::migrate, the only schema executor, which carddemo-migrate and test-support call. This
-- directory is the only schema source and modern/.sqlx is derived from it. sqlx checksums every
-- applied file, so a later change is a new numbered file, never an edit of this one.

-- ACCOUNT-RECORD, 300 bytes = RECORDSIZE(300 300) [app/jcl/ACCTFILE.jcl:41].
CREATE TABLE accounts (
    acct_id                BIGINT                NOT NULL
                           CHECK (acct_id BETWEEN 0 AND 99999999999),
    acct_active_status     CHAR(1)   COLLATE "C" NOT NULL,
    acct_curr_bal          NUMERIC(12, 2)        NOT NULL,
    acct_credit_limit      NUMERIC(12, 2)        NOT NULL,
    acct_cash_credit_limit NUMERIC(12, 2)        NOT NULL,
    acct_open_date         DATE,
    acct_expiraion_date    DATE,
    acct_reissue_date      DATE,
    acct_curr_cyc_credit   NUMERIC(12, 2)        NOT NULL,
    acct_curr_cyc_debit    NUMERIC(12, 2)        NOT NULL,
    -- Every fixture account holds its disclosure group code in acct_addr_zip and leaves
    -- acct_group_id blank (E07); both are stored as written.
    acct_addr_zip          CHAR(10)  COLLATE "C" NOT NULL,
    acct_group_id          CHAR(10)  COLLATE "C" NOT NULL,
    filler                 CHAR(178) COLLATE "C" NOT NULL,
    PRIMARY KEY (acct_id)
);

-- CUSTOMER-RECORD, 500 bytes = RECORDSIZE(500 500) [app/jcl/CUSTFILE.jcl:51].
CREATE TABLE customers (
    cust_id                  BIGINT                NOT NULL
                             CHECK (cust_id BETWEEN 0 AND 999999999),
    cust_first_name          CHAR(25)  COLLATE "C" NOT NULL,
    cust_middle_name         CHAR(25)  COLLATE "C" NOT NULL,
    cust_last_name           CHAR(25)  COLLATE "C" NOT NULL,
    cust_addr_line_1         CHAR(50)  COLLATE "C" NOT NULL,
    cust_addr_line_2         CHAR(50)  COLLATE "C" NOT NULL,
    cust_addr_line_3         CHAR(50)  COLLATE "C" NOT NULL,
    cust_addr_state_cd       CHAR(2)   COLLATE "C" NOT NULL,
    cust_addr_country_cd     CHAR(3)   COLLATE "C" NOT NULL,
    cust_addr_zip            CHAR(10)  COLLATE "C" NOT NULL,
    cust_phone_num_1         CHAR(15)  COLLATE "C" NOT NULL,
    cust_phone_num_2         CHAR(15)  COLLATE "C" NOT NULL,
    -- CUST-SSN is sensitive. Storage is not egress, so the full value is kept here; every
    -- response, log and message shows only its last four digits (Cardholder Data Redaction, E32).
    cust_ssn                 BIGINT                NOT NULL
                             CHECK (cust_ssn BETWEEN 0 AND 999999999),
    cust_govt_issued_id      CHAR(20)  COLLATE "C" NOT NULL,
    cust_dob_yyyy_mm_dd      DATE,
    cust_eft_account_id      CHAR(10)  COLLATE "C" NOT NULL,
    cust_pri_card_holder_ind CHAR(1)   COLLATE "C" NOT NULL,
    cust_fico_credit_score   BIGINT                NOT NULL
                             CHECK (cust_fico_credit_score BETWEEN 0 AND 999),
    filler                   CHAR(168) COLLATE "C" NOT NULL,
    PRIMARY KEY (cust_id)
);

-- CARD-RECORD, 150 bytes = RECORDSIZE(150 150) [app/jcl/CARDFILE.jcl:55]. CARD-NUM and
-- CARD-CVV-CD are sensitive and stored in full; only egress code redacts them (E32).
CREATE TABLE cards (
    card_num            CHAR(16) COLLATE "C" NOT NULL,
    -- CARD-ACCT-ID, under the shared account-key name.
    acct_id             BIGINT               NOT NULL
                        CHECK (acct_id BETWEEN 0 AND 99999999999),
    card_cvv_cd         BIGINT               NOT NULL
                        CHECK (card_cvv_cd BETWEEN 0 AND 999),
    card_embossed_name  CHAR(50) COLLATE "C" NOT NULL,
    card_expiraion_date DATE,
    card_active_status  CHAR(1)  COLLATE "C" NOT NULL,
    filler              CHAR(59) COLLATE "C" NOT NULL,
    -- Not a copybook field: the opaque handle that API responses and page keys carry instead of
    -- the card number (Cardholder Data Redaction, AAP 0.3.4.8). Codec B never writes it, and a
    -- reload assigns new values.
    card_ref            BIGINT GENERATED ALWAYS AS IDENTITY NOT NULL,
    PRIMARY KEY (card_num),
    CONSTRAINT cards_card_ref_key UNIQUE (card_ref)
);

-- Replaces the AIX on CARD-ACCT-ID, KEYS(11 16) NONUNIQUEKEY [app/jcl/CARDFILE.jcl:83-86]. A
-- non-unique AIX over a KSDS returns one account's cards in prime-key order, hence card_num as
-- the second column.
CREATE INDEX cards_acct_id_idx ON cards (acct_id, card_num);

-- CARD-XREF-RECORD, 50 bytes = RECORDSIZE(50 50) [app/jcl/XREFFILE.jcl:44]. XREF-CARD-NUM is
-- sensitive and stored in full (E32).
CREATE TABLE card_xref (
    card_num CHAR(16) COLLATE "C" NOT NULL,
    cust_id  BIGINT               NOT NULL
             CHECK (cust_id BETWEEN 0 AND 999999999),
    acct_id  BIGINT               NOT NULL
             CHECK (acct_id BETWEEN 0 AND 99999999999),
    -- The 36-byte cardxref.txt lines end before this FILLER, so it holds 14 spaces; the short
    -- line length lives in codec A's framing, which no column stores (E04).
    filler   CHAR(14) COLLATE "C" NOT NULL,
    PRIMARY KEY (card_num)
);

-- Replaces the AIX on XREF-ACCT-ID, KEYS(11,25) NONUNIQUEKEY [app/jcl/XREFFILE.jcl:72-75].
CREATE INDEX card_xref_acct_id_idx ON card_xref (acct_id);

-- TRAN-TYPE-RECORD, 60 bytes = RECORDSIZE(60 60) [app/jcl/TRANTYPE.jcl:41]. The VSAM data
-- version, read only by tranrept; the Db2 version is db2.transaction_type (migration 0006), and
-- no component copies between the two (E23).
CREATE TABLE transaction_types (
    tran_type      CHAR(2)  COLLATE "C" NOT NULL,
    tran_type_desc CHAR(50) COLLATE "C" NOT NULL,
    filler         CHAR(8)  COLLATE "C" NOT NULL,
    PRIMARY KEY (tran_type)
);

-- TRAN-CAT-RECORD, 60 bytes = RECORDSIZE(60 60) [app/jcl/TRANCATG.jcl:41]. The VSAM data
-- version (E23); no foreign key to transaction_types, as VSAM declares none.
CREATE TABLE transaction_categories (
    tran_type_cd       CHAR(2)  COLLATE "C" NOT NULL,
    tran_cat_cd        BIGINT               NOT NULL
                       CHECK (tran_cat_cd BETWEEN 0 AND 9999),
    tran_cat_type_desc CHAR(50) COLLATE "C" NOT NULL,
    filler             CHAR(4)  COLLATE "C" NOT NULL,
    PRIMARY KEY (tran_type_cd, tran_cat_cd)
);

-- DIS-GROUP-RECORD, 50 bytes = RECORDSIZE(50 50) [app/jcl/DISCGRP.jcl:41].
CREATE TABLE disclosure_groups (
    -- DIS-ACCT-GROUP-ID, under the shared group-key name.
    group_id     CHAR(10) COLLATE "C" NOT NULL,
    tran_type_cd CHAR(2)  COLLATE "C" NOT NULL,
    tran_cat_cd  BIGINT               NOT NULL
                 CHECK (tran_cat_cd BETWEEN 0 AND 9999),
    -- DIS-INT-RATE S9(04)V99. It is not a shared key column, so it keeps the dis_ record prefix,
    -- as copybooks.md row CVTRA02Y-004 names it.
    dis_int_rate NUMERIC(6, 2)        NOT NULL,
    filler       CHAR(28) COLLATE "C" NOT NULL,
    PRIMARY KEY (group_id, tran_type_cd, tran_cat_cd)
);

-- TRAN-CAT-BAL-RECORD, 50 bytes = RECORDSIZE(50 50) [app/jcl/TCATBALF.jcl:41].
CREATE TABLE tran_cat_balances (
    acct_id      BIGINT               NOT NULL
                 CHECK (acct_id BETWEEN 0 AND 99999999999),
    tran_type_cd CHAR(2)  COLLATE "C" NOT NULL,
    tran_cat_cd  BIGINT               NOT NULL
                 CHECK (tran_cat_cd BETWEEN 0 AND 9999),
    tran_cat_bal NUMERIC(11, 2)       NOT NULL,
    filler       CHAR(22) COLLATE "C" NOT NULL,
    PRIMARY KEY (acct_id, tran_type_cd, tran_cat_cd)
);
