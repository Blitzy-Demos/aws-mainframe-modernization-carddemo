-- Master file schema: the VSAM customer, account, card and card cross-reference files and
-- the user security file (CUSTDAT, ACCTDAT, CARDDAT with its AIX CARDAIX, CCXREF with its
-- AIX CXACAIX, USRSEC). It replaces the IDCAMS define steps of CUSTFILE, ACCTFILE, CARDFILE,
-- XREFFILE and DUSRSECJ. It holds no rows: `batch seed` loads custdata.txt, acctdata.txt,
-- carddata.txt and cardxref.txt (50 records each) and the 10 DUSRSECJ SYSUT1 records.
--
-- Sources:
--   app/cpy/CVCUS01Y.cpy, CVACT01Y.cpy, CVACT02Y.cpy, CVACT03Y.cpy, CSUSR01Y.cpy  record layouts
--   app/jcl/CUSTFILE.jcl, ACCTFILE.jcl, CARDFILE.jcl, XREFFILE.jcl, DUSRSECJ.jcl KSDS keys, record sizes, AIX keys
--   app/csd/CARDDEMO.CSD                                                        CICS files CARDAIX, CXACAIX
--   app/cbl/COUSR01C.cbl                                                        user add edits and stored values
--   docs/parity/parity-oracle-audit.md, section A                               reviewed layout rows cited below
--
-- Conventions (shared with 0001):
--   X(n) maps to VARCHAR(n) COLLATE "C". The database default collation (en_US.utf8 in
--   postgres:16.15) does not sort by byte value; "C" gives the byte order of the KSDS keys
--   (the COUSR00C user browse [app/cbl/COUSR00C.cbl:L284-311]) and of COBOL string
--   comparisons (the CBTRN02C expiration date test [app/cbl/CBTRN02C.cbl:L414]).
--   Text is stored right-trimmed of trailing spaces; the codec and persistence layers pad
--   it back to the PIC width. COBOL dates X(10) stay text.
--   Unsigned 9(n) maps to INTEGER for n <= 9 and BIGINT for n >= 10, with
--   CHECK (0 <= v < 10^n).
--   S9(10)V99 maps to NUMERIC(12,2): the precision bounds the value to the 10 integer
--   digits of the PIC and the scale to its 2 decimals. PostgreSQL NUMERIC also stores NaN,
--   which no PIC clause and no rust_decimal newtype (Money in
--   crates/domain/src/numeric/money.rs) can hold, so each money column carries
--   CHECK (v <> 'NaN') and no sign check.
--   Every column is NOT NULL, as every field of a VSAM record holds a value.
--   Text columns carry no value-domain check: COUSR01C stores any non-blank user type
--   [app/cbl/COUSR01C.cbl:L142-158], and the status flags hold whatever the files hold.
--   FILLER is not stored; the codec layouts keep its bytes.
--   No foreign keys link these tables. VSAM enforces no relationship between the files,
--   and the COBOL relies on the gaps being possible: CBTRN02C rejects reason 100 (card not
--   in the cross-reference) [app/cbl/CBTRN02C.cbl:L385-386] and 101 (account not found)
--   [app/cbl/CBTRN02C.cbl:L397-398], and the COACTVWC and COCRDSLC not-found paths.
--   Primary keys use the request's entity key names (customer_id, account_id, card_number,
--   user_id); every other column is the copybook field name lower-snake-cased, with its
--   prefix and spelling kept (acct_expiraion_date, card_expiraion_date).
--
-- customers: VSAM CUSTDAT, CVCUS01Y CUSTOMER-RECORD, RECLN 500, KEYS(9 0) [app/jcl/CUSTFILE.jcl:L50-51]
--   column                   | copybook field           | PIC    | SQL type                         | audit row
--   customer_id              | CUST-ID                  | 9(09)  | INTEGER NOT NULL, 0 to 999999999 | P-CVCUS01Y-001
--   cust_first_name          | CUST-FIRST-NAME          | X(25)  | VARCHAR(25) COLLATE "C" NOT NULL | P-CVCUS01Y-002
--   cust_middle_name         | CUST-MIDDLE-NAME         | X(25)  | VARCHAR(25) COLLATE "C" NOT NULL | P-CVCUS01Y-003
--   cust_last_name           | CUST-LAST-NAME           | X(25)  | VARCHAR(25) COLLATE "C" NOT NULL | P-CVCUS01Y-004
--   cust_addr_line_1         | CUST-ADDR-LINE-1         | X(50)  | VARCHAR(50) COLLATE "C" NOT NULL | P-CVCUS01Y-005
--   cust_addr_line_2         | CUST-ADDR-LINE-2         | X(50)  | VARCHAR(50) COLLATE "C" NOT NULL | P-CVCUS01Y-006
--   cust_addr_line_3         | CUST-ADDR-LINE-3         | X(50)  | VARCHAR(50) COLLATE "C" NOT NULL | P-CVCUS01Y-007
--   cust_addr_state_cd       | CUST-ADDR-STATE-CD       | X(02)  | VARCHAR(2) COLLATE "C" NOT NULL  | P-CVCUS01Y-008
--   cust_addr_country_cd     | CUST-ADDR-COUNTRY-CD     | X(03)  | VARCHAR(3) COLLATE "C" NOT NULL  | P-CVCUS01Y-009
--   cust_addr_zip            | CUST-ADDR-ZIP            | X(10)  | VARCHAR(10) COLLATE "C" NOT NULL | P-CVCUS01Y-010
--   cust_phone_num_1         | CUST-PHONE-NUM-1         | X(15)  | VARCHAR(15) COLLATE "C" NOT NULL | P-CVCUS01Y-011
--   cust_phone_num_2         | CUST-PHONE-NUM-2         | X(15)  | VARCHAR(15) COLLATE "C" NOT NULL | P-CVCUS01Y-012
--   cust_ssn                 | CUST-SSN                 | 9(09)  | INTEGER NOT NULL, 0 to 999999999 | P-CVCUS01Y-013
--   cust_govt_issued_id      | CUST-GOVT-ISSUED-ID      | X(20)  | VARCHAR(20) COLLATE "C" NOT NULL | P-CVCUS01Y-014
--   cust_dob_yyyy_mm_dd      | CUST-DOB-YYYY-MM-DD      | X(10)  | VARCHAR(10) COLLATE "C" NOT NULL | P-CVCUS01Y-015
--   cust_eft_account_id      | CUST-EFT-ACCOUNT-ID      | X(10)  | VARCHAR(10) COLLATE "C" NOT NULL | P-CVCUS01Y-016
--   cust_pri_card_holder_ind | CUST-PRI-CARD-HOLDER-IND | X(01)  | VARCHAR(1) COLLATE "C" NOT NULL  | P-CVCUS01Y-017
--   cust_fico_credit_score   | CUST-FICO-CREDIT-SCORE   | 9(03)  | INTEGER NOT NULL, 0 to 999       | P-CVCUS01Y-018
--   (not stored)             | FILLER                   | X(168) | not stored                       | P-CVCUS01Y-019
--
-- accounts: VSAM ACCTDAT, CVACT01Y ACCOUNT-RECORD, RECLN 300, KEYS(11 0) [app/jcl/ACCTFILE.jcl:L40-41]
--   column                 | copybook field         | PIC        | SQL type                           | audit row
--   account_id             | ACCT-ID                | 9(11)      | BIGINT NOT NULL, 0 to 99999999999  | P-CVACT01Y-001
--   acct_active_status     | ACCT-ACTIVE-STATUS     | X(01)      | VARCHAR(1) COLLATE "C" NOT NULL    | P-CVACT01Y-002
--   acct_curr_bal          | ACCT-CURR-BAL          | S9(10)V99  | NUMERIC(12,2) NOT NULL             | P-CVACT01Y-003
--   acct_credit_limit      | ACCT-CREDIT-LIMIT      | S9(10)V99  | NUMERIC(12,2) NOT NULL             | P-CVACT01Y-004
--   acct_cash_credit_limit | ACCT-CASH-CREDIT-LIMIT | S9(10)V99  | NUMERIC(12,2) NOT NULL             | P-CVACT01Y-005
--   acct_open_date         | ACCT-OPEN-DATE         | X(10)      | VARCHAR(10) COLLATE "C" NOT NULL   | P-CVACT01Y-006
--   acct_expiraion_date    | ACCT-EXPIRAION-DATE    | X(10)      | VARCHAR(10) COLLATE "C" NOT NULL   | P-CVACT01Y-007
--   acct_reissue_date      | ACCT-REISSUE-DATE      | X(10)      | VARCHAR(10) COLLATE "C" NOT NULL   | P-CVACT01Y-008
--   acct_curr_cyc_credit   | ACCT-CURR-CYC-CREDIT   | S9(10)V99  | NUMERIC(12,2) NOT NULL             | P-CVACT01Y-009
--   acct_curr_cyc_debit    | ACCT-CURR-CYC-DEBIT    | S9(10)V99  | NUMERIC(12,2) NOT NULL             | P-CVACT01Y-010
--   acct_addr_zip          | ACCT-ADDR-ZIP          | X(10)      | VARCHAR(10) COLLATE "C" NOT NULL   | P-CVACT01Y-011
--   acct_group_id          | ACCT-GROUP-ID          | X(10)      | VARCHAR(10) COLLATE "C" NOT NULL   | P-CVACT01Y-012
--   (not stored)           | FILLER                 | X(178)     | not stored                         | P-CVACT01Y-013
--   The fixture's group IDs are all blank and A000000000 sits in the ZIP field (E-06). The
--   seed reads both by their PIC offsets, so neither column carries a check.
--
-- cards: VSAM CARDDAT, CVACT02Y CARD-RECORD, RECLN 150, KEYS(16 0) [app/jcl/CARDFILE.jcl:L54-55]
--   column              | copybook field      | PIC   | SQL type                          | audit row
--   card_number         | CARD-NUM            | X(16) | VARCHAR(16) COLLATE "C" NOT NULL  | P-CVACT02Y-001
--   card_acct_id        | CARD-ACCT-ID        | 9(11) | BIGINT NOT NULL, 0 to 99999999999 | P-CVACT02Y-002
--   card_cvv_cd         | CARD-CVV-CD         | 9(03) | INTEGER NOT NULL, 0 to 999        | P-CVACT02Y-003
--   card_embossed_name  | CARD-EMBOSSED-NAME  | X(50) | VARCHAR(50) COLLATE "C" NOT NULL  | P-CVACT02Y-004
--   card_expiraion_date | CARD-EXPIRAION-DATE | X(10) | VARCHAR(10) COLLATE "C" NOT NULL  | P-CVACT02Y-005
--   card_active_status  | CARD-ACTIVE-STATUS  | X(01) | VARCHAR(1) COLLATE "C" NOT NULL   | P-CVACT02Y-006
--   (not stored)        | FILLER              | X(59) | not stored                        | P-CVACT02Y-007
--
-- card_xref: VSAM CCXREF, CVACT03Y CARD-XREF-RECORD, RECLN 50, KEYS(16 0) [app/jcl/XREFFILE.jcl:L43-44]
--   column        | copybook field | PIC   | SQL type                          | audit row
--   xref_card_num | XREF-CARD-NUM  | X(16) | VARCHAR(16) COLLATE "C" NOT NULL  | P-CVACT03Y-001
--   xref_cust_id  | XREF-CUST-ID   | 9(09) | INTEGER NOT NULL, 0 to 999999999  | P-CVACT03Y-002
--   xref_acct_id  | XREF-ACCT-ID   | 9(11) | BIGINT NOT NULL, 0 to 99999999999 | P-CVACT03Y-003
--   (not stored)  | FILLER         | X(14) | not stored                        | P-CVACT03Y-004
--   P-CVACT03Y-001 lists column=card_number; its PIC, width and offset are the ones used
--   here. The cross-reference is not one of the request's entities, so its key keeps the
--   copybook name, like xref_cust_id and xref_acct_id, and card_number names only the
--   cards key. cardxref.txt stores 36 of the 50 bytes per record (E-04); that framing is
--   the codec's concern and does not reach this table.
--
-- users: VSAM USRSEC, CSUSR01Y SEC-USER-DATA, RECLN 80, KEYS(8,0) [app/jcl/DUSRSECJ.jcl:L64-66]
--   column        | copybook field | PIC   | SQL type                         | audit row
--   user_id       | SEC-USR-ID     | X(08) | VARCHAR(8) COLLATE "C" NOT NULL  | P-CSUSR01Y-001
--   sec_usr_fname | SEC-USR-FNAME  | X(20) | VARCHAR(20) COLLATE "C" NOT NULL | P-CSUSR01Y-002
--   sec_usr_lname | SEC-USR-LNAME  | X(20) | VARCHAR(20) COLLATE "C" NOT NULL | P-CSUSR01Y-003
--   password_hash | SEC-USR-PWD    | X(08) | TEXT NOT NULL, argon2id PHC      | P-CSUSR01Y-004
--   sec_usr_type  | SEC-USR-TYPE   | X(01) | VARCHAR(1) COLLATE "C" NOT NULL  | P-CSUSR01Y-005
--   (not stored)  | SEC-USR-FILLER | X(23) | not stored                       | P-CSUSR01Y-006
--   No plaintext password column exists. password_hash holds the argon2id PHC string of
--   the exact 8 bytes the admin programs store: the password as entered, case unchanged,
--   space-padded to 8 [app/cbl/COUSR01C.cbl:L154-158] (E-20). Sign-on upper-cases the entered
--   password before verifying it, in crates/domain/src/auth/password.rs, never here.
--   GET /admin/users/{id} never returns a password, so the COUSR02C display of the current
--   password has no counterpart (E-21). P-CSUSR01Y-004 lists sql=VARCHAR (PHC string);
--   unbounded VARCHAR and TEXT are the same PostgreSQL string type, and TEXT is used.

CREATE TABLE customers (
    customer_id              INTEGER                 NOT NULL PRIMARY KEY
        CHECK (customer_id >= 0 AND customer_id < 1000000000),
    cust_first_name          VARCHAR(25) COLLATE "C" NOT NULL,
    cust_middle_name         VARCHAR(25) COLLATE "C" NOT NULL,
    cust_last_name           VARCHAR(25) COLLATE "C" NOT NULL,
    cust_addr_line_1         VARCHAR(50) COLLATE "C" NOT NULL,
    cust_addr_line_2         VARCHAR(50) COLLATE "C" NOT NULL,
    cust_addr_line_3         VARCHAR(50) COLLATE "C" NOT NULL,
    cust_addr_state_cd       VARCHAR(2)  COLLATE "C" NOT NULL,
    cust_addr_country_cd     VARCHAR(3)  COLLATE "C" NOT NULL,
    cust_addr_zip            VARCHAR(10) COLLATE "C" NOT NULL,
    cust_phone_num_1         VARCHAR(15) COLLATE "C" NOT NULL,
    cust_phone_num_2         VARCHAR(15) COLLATE "C" NOT NULL,
    cust_ssn                 INTEGER                 NOT NULL
        CHECK (cust_ssn >= 0 AND cust_ssn < 1000000000),
    cust_govt_issued_id      VARCHAR(20) COLLATE "C" NOT NULL,
    cust_dob_yyyy_mm_dd      VARCHAR(10) COLLATE "C" NOT NULL,
    cust_eft_account_id      VARCHAR(10) COLLATE "C" NOT NULL,
    cust_pri_card_holder_ind VARCHAR(1)  COLLATE "C" NOT NULL,
    cust_fico_credit_score   INTEGER                 NOT NULL
        CHECK (cust_fico_credit_score >= 0 AND cust_fico_credit_score < 1000)
);

-- migrations/0005_rewards.sql references accounts (account_id) from reward_balances, so a
-- seed refresh clears the reward tables before it reloads this one.
CREATE TABLE accounts (
    account_id             BIGINT                  NOT NULL PRIMARY KEY
        CHECK (account_id >= 0 AND account_id < 100000000000),
    acct_active_status     VARCHAR(1)  COLLATE "C" NOT NULL,
    acct_curr_bal          NUMERIC(12,2)           NOT NULL
        CHECK (acct_curr_bal <> 'NaN'),
    acct_credit_limit      NUMERIC(12,2)           NOT NULL
        CHECK (acct_credit_limit <> 'NaN'),
    acct_cash_credit_limit NUMERIC(12,2)           NOT NULL
        CHECK (acct_cash_credit_limit <> 'NaN'),
    acct_open_date         VARCHAR(10) COLLATE "C" NOT NULL,
    acct_expiraion_date    VARCHAR(10) COLLATE "C" NOT NULL,
    acct_reissue_date      VARCHAR(10) COLLATE "C" NOT NULL,
    acct_curr_cyc_credit   NUMERIC(12,2)           NOT NULL
        CHECK (acct_curr_cyc_credit <> 'NaN'),
    acct_curr_cyc_debit    NUMERIC(12,2)           NOT NULL
        CHECK (acct_curr_cyc_debit <> 'NaN'),
    acct_addr_zip          VARCHAR(10) COLLATE "C" NOT NULL,
    acct_group_id          VARCHAR(10) COLLATE "C" NOT NULL
);

CREATE TABLE cards (
    card_number         VARCHAR(16) COLLATE "C" NOT NULL PRIMARY KEY,
    card_acct_id        BIGINT                  NOT NULL
        CHECK (card_acct_id >= 0 AND card_acct_id < 100000000000),
    card_cvv_cd         INTEGER                 NOT NULL
        CHECK (card_cvv_cd >= 0 AND card_cvv_cd < 1000),
    card_embossed_name  VARCHAR(50) COLLATE "C" NOT NULL,
    card_expiraion_date VARCHAR(10) COLLATE "C" NOT NULL,
    card_active_status  VARCHAR(1)  COLLATE "C" NOT NULL
);

-- Replaces the non-unique alternate index KEYS(11 16) on CARD-ACCT-ID
-- [app/jcl/CARDFILE.jcl:L83-86], opened through the CICS file CARDAIX
-- [app/csd/CARDDEMO.CSD:L13-14].
CREATE INDEX cardaix ON cards (card_acct_id);

CREATE TABLE card_xref (
    xref_card_num VARCHAR(16) COLLATE "C" NOT NULL PRIMARY KEY,
    xref_cust_id  INTEGER                 NOT NULL
        CHECK (xref_cust_id >= 0 AND xref_cust_id < 1000000000),
    xref_acct_id  BIGINT                  NOT NULL
        CHECK (xref_acct_id >= 0 AND xref_acct_id < 100000000000)
);

-- Replaces the non-unique alternate index KEYS(11,25) on XREF-ACCT-ID
-- [app/jcl/XREFFILE.jcl:L72-75], opened through the CICS file CXACAIX
-- [app/csd/CARDDEMO.CSD:L63-65].
CREATE INDEX cxacaix ON card_xref (xref_acct_id);

-- The prefix check enforces only the '$argon2id$' prefix of the PHC string format, not the
-- rest of its syntax, so '$argon2id$garbage' passes it. The full format is owned by
-- crates/domain/src/auth/password.rs: hash_stored_password generates every stored value as
-- the PHC string $argon2id$v=19$m=19456,t=2,p=1$<salt>$<hash>, and verify_padded parses the
-- stored value with PasswordHash::new, rejecting one that does not parse or lacks a salt or
-- hash output as PasswordError::MalformedHash. The prefix alone is 10 bytes and SEC-USR-PWD
-- is X(08), so no plaintext SEC-USR-PWD value can be stored.
CREATE TABLE users (
    user_id       VARCHAR(8)  COLLATE "C" NOT NULL PRIMARY KEY,
    sec_usr_fname VARCHAR(20) COLLATE "C" NOT NULL,
    sec_usr_lname VARCHAR(20) COLLATE "C" NOT NULL,
    password_hash TEXT                    NOT NULL
        CHECK (password_hash LIKE '$argon2id$%'),
    sec_usr_type  VARCHAR(1)  COLLATE "C" NOT NULL
);
