-- Db2 data version of the transaction types and categories, read and written only by the ports of
-- COTRTLIC, COTRTUPC and COBTUPDT.
--
-- This file performs CREADB21's effect [app/app-transaction-type-db2/jcl/CREADB21.jcl:52-84]: step
-- CRCRDDB creates the two tables, then steps RUNTEP2 and LDTCCAT run the seed members DB2LTTYP and
-- DB2LTCAT. sqlx records the migration as applied, so it runs once per database, and a fixture
-- reload neither seeds nor clears these tables.
--
-- Sources (Db2 types kept as declared):
--   [app/app-transaction-type-db2/ddl/TRNTYPE.ddl:1-4]     CARDDEMO.TRANSACTION_TYPE
--   [app/app-transaction-type-db2/ddl/TRNTYCAT.ddl:1-7]    CARDDEMO.TRANSACTION_TYPE_CATEGORY
--   [app/app-transaction-type-db2/ddl/XTRNTYPE.ddl:1-5]    unique index XTRAN_TYPE
--   [app/app-transaction-type-db2/ddl/XTRNTYCAT.ddl:1-5]   unique index X_TRAN_TYPE_CATG
--   [app/app-transaction-type-db2/ctl/DB2LTTYP.ctl:15-23]  the 7 type rows
--   [app/app-transaction-type-db2/ctl/DB2LTCAT.ctl:15-37]  the 18 category rows
--   app/app-transaction-type-db2/dcl/{DCLTRTYP,DCLTRCAT}.dcl  the DCLGEN layouts of both tables
-- Schema db2 stands in for the qualifier CARDDEMO. The database, tablespace, storage-group and
-- GRANT statements of CREADB21's DB2CREAT member have no PostgreSQL counterpart.
--
-- E23 (modern/audit/ESCALATIONS.md): this version differs on purpose from the VSAM version in
-- transaction_types and transaction_categories (migration 0001), for example PURCHASE and REVERAL
-- here against Purchase and Reversal in [app/data/ASCII/trantype.txt:1,6]. On the mainframe only
-- the daily TRANEXTR copied the Db2 version into the VSAM one. TRANEXTR is out of scope, and no
-- component copies between the versions, so maintenance here never reaches the TRANREPT lookups.
--
-- Every character column is COLLATE "C": Db2 compares and orders keys by their bytes, COTRTLIC
-- pages on XTRAN_TYPE, and the server's default locale must not change that order. The Db2 database
-- was CCSID EBCDIC; the type codes COTRTUPC stores are two digits
-- [app/app-transaction-type-db2/cbl/COTRTUPC.cbl:820-846], which order alike in EBCDIC and ASCII.
--
-- The seed values are unpadded VARCHAR literals copied verbatim from the CTL members, in their
-- order; REVERAL is the source's spelling. The members' COMMIT statements are not copied: sqlx
-- applies this file in one transaction. This directory is the only schema source; a later change is
-- a new numbered file, never an edit of this one.

CREATE SCHEMA db2;

-- TR_DESCRIPTION is VARCHAR in Db2 (DCL-TR-DESCRIPTION-LEN plus -TEXT in DCLTRTYP), so its length,
-- not a space padding, is part of the stored value.
CREATE TABLE db2.transaction_type (
    tr_type        CHAR(2)     COLLATE "C" NOT NULL,
    tr_description VARCHAR(50) COLLATE "C" NOT NULL,
    -- The primary-key index takes the name of XTRNTYPE.ddl's unique index on the same key.
    CONSTRAINT xtran_type PRIMARY KEY (tr_type)
);

-- TRC_TYPE_CATEGORY stays CHAR(4), as the DDL declares it, unlike the VSAM tran_cat_cd 9(04).
CREATE TABLE db2.transaction_type_category (
    trc_type_code     CHAR(2)     COLLATE "C" NOT NULL,
    trc_type_category CHAR(4)     COLLATE "C" NOT NULL,
    trc_cat_data      VARCHAR(50) COLLATE "C" NOT NULL,
    -- The primary-key index takes the name of XTRNTYCAT.ddl's unique index on the same key.
    CONSTRAINT x_tran_type_catg PRIMARY KEY (trc_type_code, trc_type_category),
    -- TRNTYCAT.ddl names this foreign key TRC_TYPE_CODE. RESTRICT makes deleting a type that still
    -- has categories fail, the SQLCODE -532 path of COTRTUPC and COTRTLIC.
    CONSTRAINT trc_type_code FOREIGN KEY (trc_type_code)
        REFERENCES db2.transaction_type (tr_type) ON DELETE RESTRICT
);

-- [app/app-transaction-type-db2/ctl/DB2LTTYP.ctl:15-23]
INSERT INTO db2.transaction_type (tr_type, tr_description) VALUES
    ('01', 'PURCHASE'),
    ('02', 'PAYMENT'),
    ('03', 'CREDIT'),
    ('04', 'AUTHORIZATION'),
    ('05', 'REFUND'),
    ('06', 'REVERAL'),
    ('07', 'ADJUSTMENT');

-- [app/app-transaction-type-db2/ctl/DB2LTCAT.ctl:15-37]
INSERT INTO db2.transaction_type_category (trc_type_code, trc_type_category, trc_cat_data) VALUES
    ('01', '0001', 'REGULAR SALES DRAFT'),
    ('01', '0002', 'REGULAR CASH ADVANCE'),
    ('01', '0003', 'CONVENIENCE CHECK DEBIT'),
    ('01', '0004', 'ATM CASH ADVANCE'),
    ('01', '0005', 'INTEREST AMOUNT'),
    ('02', '0001', 'CASH PAYMENT'),
    ('02', '0002', 'ELECTRONIC PAYMENT'),
    ('02', '0003', 'CHECK PAYMENT'),
    ('03', '0001', 'CREDIT TO ACCOUNT'),
    ('03', '0002', 'CREDIT TO PURCHASE BALANCE'),
    ('03', '0003', 'CREDIT TO CASH BALANCE'),
    ('04', '0001', 'ZERO DOLLAR AUTHORIZATION'),
    ('04', '0002', 'ONLINE PURCHASE AUTHORIZATION'),
    ('04', '0003', 'TRAVEL BOOKING AUTHORIZATION'),
    ('05', '0001', 'REFUND CREDIT'),
    ('06', '0001', 'FRAUD REVERSAL'),
    ('06', '0002', 'NON FRAUD REVERSAL'),
    ('07', '0001', 'SALES DRAFT CREDIT ADJUSTMENT');
