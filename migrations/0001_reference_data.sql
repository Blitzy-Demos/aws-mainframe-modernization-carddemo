-- Reference data schema: the Db2 transaction type and category masters, their VSAM
-- processing copies TRANTYPE and TRANCATG, and the disclosure group file (CREADB21 role).
--
-- Sources:
--   app/app-transaction-type-db2/ddl/TRNTYPE.ddl, TRNTYCAT.ddl   tables, keys, foreign key
--   app/app-transaction-type-db2/ddl/XTRNTYPE.ddl, XTRNTYCAT.ddl unique key indexes
--   app/app-transaction-type-db2/dcl/DCLTRTYP.dcl, DCLTRCAT.dcl  DCLGEN host variables
--   app/app-transaction-type-db2/jcl/CREADB21.jcl               create, then DB2LTTYP and DB2LTCAT loads
--   app/cpy/CVTRA02Y.cpy, CVTRA03Y.cpy, CVTRA04Y.cpy            record layouts
--   app/jcl/DISCGRP.jcl, TRANTYPE.jcl, TRANCATG.jcl             KSDS keys and record sizes
--   docs/parity/parity-oracle-audit.md, section A               reviewed layout rows cited below
--
-- Conventions:
--   X(n), Db2 CHAR(n) and Db2 VARCHAR(n) map to VARCHAR(n) COLLATE "C". The database
--   default collation (en_US.utf8 in postgres:16.15) does not sort by byte value; "C"
--   gives the byte order of the COBOL keys and of COBOL string comparisons.
--   Text is stored right-trimmed of trailing spaces; the codec and persistence layers pad
--   it back to the PIC width.
--   Every VARCHAR column carries CHECK (v = rtrim(v) AND v !~ '[^\x01-\xff]'), which
--   admits only that stored form. A value must equal its rtrim, so a trailing-space
--   variant is rejected and one COBOL key is one row: PostgreSQL VARCHAR equality counts
--   trailing spaces, where COBOL and Db2 comparisons pad the shorter operand with spaces,
--   so '1' and '1 ' would otherwise be two tr_type keys for the one COBOL key. Each
--   character must lie in U+0001..U+00FF, the ISO 8859-1 range that Alnum::from_trimmed
--   (crates/domain/src/numeric/alnum.rs) reads back one byte per character; NUL cannot
--   be stored in PostgreSQL text at all.
--   Unsigned 9(n) with n <= 9 maps to INTEGER with CHECK (0 <= v < 10^n).
--   S9(04)V99 maps to NUMERIC(6,2) with CHECK (v <> 'NaN'): PostgreSQL NUMERIC also stores
--   NaN, which no PIC clause and no rust_decimal newtype (InterestRate in
--   crates/domain/src/numeric/rates.rs) can hold.
--   FILLER is not stored; the codec layouts keep its bytes.
--
-- The masters and the processing copies are separate tables with no link between them.
-- They start different (the Db2 load holds 'PURCHASE', trantype.txt holds 'Purchase') and
-- agree only after `batch seed --only tranextr` copies the masters over the processing
-- copies, as TRANEXTR does. CTTU, CTLI and MNTTRDB2 change only the masters; CBTRN03C
-- (CR00 and TRANREPT) reads only the processing copies.
--
-- transaction_types: Db2 CARDDEMO.TRANSACTION_TYPE
--   column         | DDL field      | DDL type             | SQL type                         | audit row
--   tr_type        | TR_TYPE        | CHAR(2) NOT NULL     | VARCHAR(2) COLLATE "C" NOT NULL  | P-TRNTYPE-001
--   tr_description | TR_DESCRIPTION | VARCHAR(50) NOT NULL | VARCHAR(50) COLLATE "C" NOT NULL | P-TRNTYPE-002
--
-- transaction_categories: Db2 CARDDEMO.TRANSACTION_TYPE_CATEGORY
--   column            | DDL field         | DDL type             | SQL type                         | audit row
--   trc_type_code     | TRC_TYPE_CODE     | CHAR(2) NOT NULL     | VARCHAR(2) COLLATE "C" NOT NULL  | P-TRNTYCAT-001
--   trc_type_category | TRC_TYPE_CATEGORY | CHAR(4) NOT NULL     | VARCHAR(4) COLLATE "C" NOT NULL  | P-TRNTYCAT-002
--   trc_cat_data      | TRC_CAT_DATA      | VARCHAR(50) NOT NULL | VARCHAR(50) COLLATE "C" NOT NULL | P-TRNTYCAT-003
--   P-TRNTYCAT-002 lists sql=CHAR(4); its field, width and nullability are the ones used
--   here. The column is VARCHAR(4) like every other Db2 CHAR(n) and X(n) code (the DCLGEN
--   host variable is X(4)), so every text column follows the one right-trimmed rule.
--
-- trantype_file: VSAM TRANTYPE processing copy, CVTRA03Y TRAN-TYPE-RECORD, RECLN 60, KEYS(2 0)
--   column         | copybook field | PIC    | SQL type                         | audit row
--   tran_type      | TRAN-TYPE      | X(02)  | VARCHAR(2) COLLATE "C" NOT NULL  | P-CVTRA03Y-001
--   tran_type_desc | TRAN-TYPE-DESC | X(50)  | VARCHAR(50) COLLATE "C" NOT NULL | P-CVTRA03Y-002
--   (not stored)   | FILLER         | X(08)  | not stored                       | P-CVTRA03Y-003
--
-- trancatg_file: VSAM TRANCATG processing copy, CVTRA04Y TRAN-CAT-RECORD, RECLN 60, KEYS(6 0)
--   column             | copybook field     | PIC   | SQL type                         | audit row
--   tran_type_cd       | TRAN-TYPE-CD       | X(02) | VARCHAR(2) COLLATE "C" NOT NULL  | P-CVTRA04Y-001
--   tran_cat_cd        | TRAN-CAT-CD        | 9(04) | INTEGER NOT NULL, 0 to 9999      | P-CVTRA04Y-002
--   tran_cat_type_desc | TRAN-CAT-TYPE-DESC | X(50) | VARCHAR(50) COLLATE "C" NOT NULL | P-CVTRA04Y-003
--   (not stored)       | FILLER             | X(04) | not stored                       | P-CVTRA04Y-004
--
-- disclosure_groups: VSAM DISCGRP, CVTRA02Y DIS-GROUP-RECORD, RECLN 50, KEYS(16 0)
--   column            | copybook field    | PIC       | SQL type                         | audit row
--   dis_acct_group_id | DIS-ACCT-GROUP-ID | X(10)     | VARCHAR(10) COLLATE "C" NOT NULL | P-CVTRA02Y-001
--   dis_tran_type_cd  | DIS-TRAN-TYPE-CD  | X(02)     | VARCHAR(2) COLLATE "C" NOT NULL  | P-CVTRA02Y-002
--   dis_tran_cat_cd   | DIS-TRAN-CAT-CD   | 9(04)     | INTEGER NOT NULL, 0 to 9999      | P-CVTRA02Y-003
--   dis_int_rate      | DIS-INT-RATE      | S9(04)V99 | NUMERIC(6,2) NOT NULL            | P-CVTRA02Y-004
--   (not stored)      | FILLER            | X(28)     | not stored                       | P-CVTRA02Y-005

-- Db2 masters. Both start empty: `batch seed` loads the 7 DB2LTTYP rows and the 18 DB2LTCAT
-- rows, whose descriptions are upper case ('PURCHASE', 'REGULAR SALES DRAFT').

-- The primary key index replaces the Db2 unique index XTRAN_TYPE (TR_TYPE ASC).
CREATE TABLE transaction_types (
    tr_type        VARCHAR(2)  COLLATE "C" NOT NULL
        CHECK (tr_type = rtrim(tr_type) AND tr_type !~ '[^\x01-\xff]'),
    tr_description VARCHAR(50) COLLATE "C" NOT NULL
        CHECK (tr_description = rtrim(tr_description) AND tr_description !~ '[^\x01-\xff]'),
    CONSTRAINT xtran_type PRIMARY KEY (tr_type)
);

-- COTRTLIC browses the types through the cursors C-TR-TYPE-FORWARD (TR_TYPE >= the start
-- key, ORDER BY TR_TYPE) and C-TR-TYPE-BACKWARD (TR_TYPE < the start key, ORDER BY TR_TYPE
-- DESC) [app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L338-367]. TR_TYPE is CHAR(2), so
-- that order is the byte order of the code padded with spaces to its 2 bytes. The primary
-- key on the right-trimmed column cannot supply it: it puts 'A' before 'A' || chr(1), where
-- the padded bytes put the control character, below the space, first. browse_tran_types
-- (crates/persistence/src/repos/tran_types.rs) therefore compares and orders by
-- rpad(tr_type, 2) COLLATE "C". This expression index supplies that order and the start-key
-- bound; the primary key still serves the keyed reads, updates and deletes and the equality
-- of the type filter.
CREATE INDEX transaction_types_padded_tr_type ON transaction_types ((rpad(tr_type, 2)) COLLATE "C");

-- The primary key index replaces the Db2 unique index X_TRAN_TYPE_CATG (TRC_TYPE_CODE ASC,
-- TRC_TYPE_CATEGORY ASC). As in Db2, a category whose type is missing cannot be inserted
-- (Db2 SQLCODE -530) and a type that still has categories cannot be deleted (Db2 SQLCODE
-- -532); PostgreSQL reports both as SQLSTATE 23503, which persistence maps to the audited
-- COTRTLIC and COBTUPDT messages.
CREATE TABLE transaction_categories (
    trc_type_code     VARCHAR(2)  COLLATE "C" NOT NULL
        CHECK (trc_type_code = rtrim(trc_type_code) AND trc_type_code !~ '[^\x01-\xff]'),
    trc_type_category VARCHAR(4)  COLLATE "C" NOT NULL
        CHECK (trc_type_category = rtrim(trc_type_category) AND trc_type_category !~ '[^\x01-\xff]'),
    trc_cat_data      VARCHAR(50) COLLATE "C" NOT NULL
        CHECK (trc_cat_data = rtrim(trc_cat_data) AND trc_cat_data !~ '[^\x01-\xff]'),
    CONSTRAINT x_tran_type_catg PRIMARY KEY (trc_type_code, trc_type_category),
    CONSTRAINT transaction_categories_trc_type_code_fkey
        FOREIGN KEY (trc_type_code) REFERENCES transaction_types (tr_type) ON DELETE RESTRICT
);

-- VSAM processing copies, loaded by `batch seed` from trantype.txt (7 records) and
-- trancatg.txt (18 records). VSAM declares no relationship between the two files, so
-- trancatg_file has no foreign key to trantype_file.

CREATE TABLE trantype_file (
    tran_type      VARCHAR(2)  COLLATE "C" NOT NULL PRIMARY KEY
        CHECK (tran_type = rtrim(tran_type) AND tran_type !~ '[^\x01-\xff]'),
    tran_type_desc VARCHAR(50) COLLATE "C" NOT NULL
        CHECK (tran_type_desc = rtrim(tran_type_desc) AND tran_type_desc !~ '[^\x01-\xff]')
);

-- The 6-byte KSDS key is TRAN-TYPE-CD X(02) followed by TRAN-CAT-CD 9(04). Four zoned
-- digits sort by byte exactly as their integer values sort, so this key orders rows as the
-- KSDS does.
CREATE TABLE trancatg_file (
    tran_type_cd       VARCHAR(2)  COLLATE "C" NOT NULL
        CHECK (tran_type_cd = rtrim(tran_type_cd) AND tran_type_cd !~ '[^\x01-\xff]'),
    tran_cat_cd        INTEGER                 NOT NULL CHECK (tran_cat_cd >= 0 AND tran_cat_cd < 10000),
    tran_cat_type_desc VARCHAR(50) COLLATE "C" NOT NULL
        CHECK (tran_cat_type_desc = rtrim(tran_cat_type_desc) AND tran_cat_type_desc !~ '[^\x01-\xff]'),
    PRIMARY KEY (tran_type_cd, tran_cat_cd)
);

-- Disclosure groups, loaded by `batch seed` from discgrp.txt (51 records). CBACT04C
-- 1200-GET-INTEREST-RATE reads by the full 16-byte key and falls back to group DEFAULT on
-- status 23. dis_int_rate is signed, as the InterestRate newtype is; NUMERIC(6,2) bounds it
-- to the S9(04)V99 range, and its CHECK rejects NaN, which InterestRate cannot hold. No
-- sign check applies.
CREATE TABLE disclosure_groups (
    dis_acct_group_id VARCHAR(10) COLLATE "C" NOT NULL
        CHECK (dis_acct_group_id = rtrim(dis_acct_group_id) AND dis_acct_group_id !~ '[^\x01-\xff]'),
    dis_tran_type_cd  VARCHAR(2)  COLLATE "C" NOT NULL
        CHECK (dis_tran_type_cd = rtrim(dis_tran_type_cd) AND dis_tran_type_cd !~ '[^\x01-\xff]'),
    dis_tran_cat_cd   INTEGER                 NOT NULL CHECK (dis_tran_cat_cd >= 0 AND dis_tran_cat_cd < 10000),
    dis_int_rate      NUMERIC(6,2)            NOT NULL CHECK (dis_int_rate <> 'NaN'),
    PRIMARY KEY (dis_acct_group_id, dis_tran_type_cd, dis_tran_cat_cd)
);
